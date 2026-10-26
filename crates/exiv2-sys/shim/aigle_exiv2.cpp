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
        if (packet.empty() && !image->xmpData().empty()) {
            if (Exiv2::XmpParser::encode(packet, image->xmpData()) != 0) {
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

// Look up `n` keys (`Xmp.prefix.Name`) in `packet`; `values[i]` is NULL when absent.
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
            if (it != xmp.end()) values[i] = dup(it->toString());
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

// Copy of `packet` (may be NULL or empty) with `n` simple text properties replaced;
// `values[i] == NULL` removes key `i`. Every other property is kept.
int aigle_exiv2_update(const char* packet, size_t n, const char* const* keys,
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
            if (it != xmp.end()) xmp.erase(it);
            if (values[i]) {
                Exiv2::XmpTextValue v(values[i]);
                xmp.add(key, &v);
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
