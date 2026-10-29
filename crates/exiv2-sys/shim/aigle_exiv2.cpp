// Narrow C interface over Exiv2's XMP support for aigle sidecars (doc 01 §9).
//
// Callers serialise all calls (the Rust side holds a lock); Exiv2's XMP toolkit is not
// re-entrant. Strings returned through `char**` are malloc'ed and freed with
// aigle_exiv2_free.

#include <exiv2/exiv2.hpp>

#include <cstdlib>
#include <cstring>
#include <exception>
#include <string>

namespace {

const char ITEM_SEP = '\x1f';

char* dup(const std::string& s) {
    char* p = static_cast<char*>(std::malloc(s.size() + 1));
    if (p) std::memcpy(p, s.c_str(), s.size() + 1);
    return p;
}

void set_err(char** err, const char* what) {
    if (err) *err = dup(what ? what : "unknown Exiv2 error");
}

}  // namespace

extern "C" {

int aigle_exiv2_init(const char* ns_uri, const char* ns_prefix) {
    try {
        Exiv2::LogMsg::setLevel(Exiv2::LogMsg::mute);
#ifdef EXV_ENABLE_BMFF
        Exiv2::enableBMFF(true);
#endif
        if (!Exiv2::XmpParser::initialize()) return 0;
        Exiv2::XmpProperties::registerNs(ns_uri, ns_prefix);
        return 1;
    } catch (...) {
        return 0;
    }
}

void aigle_exiv2_free(char* p) { std::free(p); }

// The XMP of `path`: a sidecar's packet, or the XMP embedded in an image. `*out` is NULL
// when the file has none. Returns 0 on success.
int aigle_exiv2_read_file(const char* path, char** out, char** err) {
    *out = nullptr;
    try {
        auto image = Exiv2::ImageFactory::open(path);
        image->readMetadata();
        std::string packet = image->xmpPacket();
        Exiv2::XmpData xmp = image->xmpData();
        if (packet.empty() && xmp.empty() && !image->iptcData().empty()) {
            Exiv2::copyIptcToXmp(image->iptcData(), xmp);
        }
        if (packet.empty() && !xmp.empty()) {
            if (Exiv2::XmpParser::encode(packet, xmp) != 0) {
                set_err(err, "cannot encode XMP");
                return 1;
            }
        }
        if (!packet.empty()) *out = dup(packet);
        return 0;
    } catch (const std::exception& e) {
        set_err(err, e.what());
        return 1;
    } catch (...) {
        set_err(err, nullptr);
        return 1;
    }
}

// Camera make, model, lens name and capture date (`YYYY:MM:DD HH:MM:SS`) from the EXIF of
// the image at `path`, as `out[0..4]`; an entry is NULL when unknown. Returns 0 on success.
int aigle_exiv2_read_facts(const char* path, char** out, char** err) {
    for (int i = 0; i < 4; ++i) out[i] = nullptr;
    try {
        auto image = Exiv2::ImageFactory::open(path);
        image->readMetadata();
        const Exiv2::ExifData& ed = image->exifData();
        if (ed.empty()) return 0;
        auto put = [&](int i, Exiv2::ExifData::const_iterator it, bool print) {
            if (it == ed.end()) return;
            std::string s = print ? it->print(&ed) : it->toString();
            if (!s.empty()) out[i] = dup(s);
        };
        put(0, Exiv2::make(ed), false);
        put(1, Exiv2::model(ed), false);
        put(2, Exiv2::lensName(ed), true);
        put(3, Exiv2::dateTimeOriginal(ed), false);
        return 0;
    } catch (const std::exception& e) {
        set_err(err, e.what());
        return 1;
    } catch (...) {
        set_err(err, nullptr);
        return 1;
    }
}

// Look up `n` keys (`Xmp.prefix.Name`) in `packet`; `values[i]` is NULL when absent. Array
// items are joined with ITEM_SEP; a language alternative gives its x-default (or first) text.
int aigle_exiv2_get(const char* packet, size_t n, const char* const* keys, char** values,
                    char** err) {
    for (size_t i = 0; i < n; ++i) values[i] = nullptr;
    try {
        Exiv2::XmpData xmp;
        if (Exiv2::XmpParser::decode(xmp, packet ? packet : "") != 0) {
            set_err(err, "malformed XMP packet");
            return 1;
        }
        for (size_t i = 0; i < n; ++i) {
            auto it = xmp.findKey(Exiv2::XmpKey(keys[i]));
            if (it == xmp.end()) continue;
            const Exiv2::Value& v = it->value();
            switch (v.typeId()) {
                case Exiv2::langAlt: {
                    const auto& alt = dynamic_cast<const Exiv2::LangAltValue&>(v);
                    auto d = alt.value_.find("x-default");
                    if (d == alt.value_.end()) d = alt.value_.begin();
                    if (d != alt.value_.end()) values[i] = dup(d->second);
                    break;
                }
                case Exiv2::xmpBag:
                case Exiv2::xmpSeq:
                case Exiv2::xmpAlt: {
                    std::string joined;
                    for (long k = 0; k < static_cast<long>(v.count()); ++k) {
                        if (k > 0) joined += ITEM_SEP;
                        joined += v.toString(k);
                    }
                    values[i] = dup(joined);
                    break;
                }
                default:
                    values[i] = dup(it->toString());
            }
        }
        return 0;
    } catch (const std::exception& e) {
        set_err(err, e.what());
        return 1;
    } catch (...) {
        set_err(err, nullptr);
        return 1;
    }
}

// Copy of `packet` (may be NULL or empty) with `n` properties replaced; `values[i] == NULL`
// removes key `i`. `kinds[i]`: 0 text, 1 bag, 2 seq (items separated by ITEM_SEP), 3 language
// alternative (sets x-default and keeps the other languages). Every other property is kept.
int aigle_exiv2_update(const char* packet, size_t n, const char* const* keys, const int* kinds,
                       const char* const* values, char** out, char** err) {
    *out = nullptr;
    try {
        Exiv2::XmpData xmp;
        if (packet && *packet && Exiv2::XmpParser::decode(xmp, packet) != 0) {
            set_err(err, "malformed XMP packet");
            return 1;
        }
        for (size_t i = 0; i < n; ++i) {
            Exiv2::XmpKey key(keys[i]);
            auto it = xmp.findKey(key);
            Exiv2::LangAltValue alt;
            if (it != xmp.end()) {
                if (kinds[i] == 3 && it->value().typeId() == Exiv2::langAlt) {
                    alt.value_ = dynamic_cast<const Exiv2::LangAltValue&>(it->value()).value_;
                }
                xmp.erase(it);
            }
            if (!values[i]) continue;
            switch (kinds[i]) {
                case 1:
                case 2: {
                    Exiv2::XmpArrayValue v(kinds[i] == 1 ? Exiv2::xmpBag : Exiv2::xmpSeq);
                    std::string all(values[i]);
                    size_t start = 0;
                    while (start <= all.size()) {
                        size_t end = all.find(ITEM_SEP, start);
                        if (end == std::string::npos) end = all.size();
                        v.read(all.substr(start, end - start));
                        start = end + 1;
                    }
                    xmp.add(key, &v);
                    break;
                }
                case 3:
                    alt.value_["x-default"] = values[i];
                    xmp.add(key, &alt);
                    break;
                default: {
                    Exiv2::XmpTextValue v(values[i]);
                    xmp.add(key, &v);
                }
            }
        }
        std::string encoded;
        if (Exiv2::XmpParser::encode(encoded, xmp, Exiv2::XmpParser::useCompactFormat) != 0) {
            set_err(err, "cannot encode XMP");
            return 1;
        }
        *out = dup(encoded);
        return 0;
    } catch (const std::exception& e) {
        set_err(err, e.what());
        return 1;
    } catch (...) {
        set_err(err, nullptr);
        return 1;
    }
}

}  // extern "C"
