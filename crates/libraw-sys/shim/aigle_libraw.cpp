// aigle shim over LibRaw. See aigle_libraw.h.

#include "aigle_libraw.h"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <ctime>
#include <new>

#include "libraw/libraw.h"

namespace {

// First value of a TIFF field of `type` in byte order `order` (0x4949 = little endian).
double tiff_real(short order, int type, const unsigned char *b) {
  auto u = [&](int at, int n) {
    uint64_t v = 0;
    for (int i = 0; i < n; i++)
      v |= uint64_t(b[at + (order == 0x4949 ? i : n - 1 - i)]) << (8 * i);
    return v;
  };
  switch (type) {
  case 1:
    return b[0];
  case 6:
    return int8_t(b[0]);
  case 3:
    return double(u(0, 2));
  case 8:
    return int16_t(u(0, 2));
  case 4:
    return double(u(0, 4));
  case 9:
    return int32_t(u(0, 4));
  case 5:
  case 10: {
    double n = type == 5 ? double(u(0, 4)) : double(int32_t(u(0, 4)));
    double d = type == 5 ? double(u(4, 4)) : double(int32_t(u(4, 4)));
    return d != 0 ? n / d : NAN;
  }
  case 11: {
    uint32_t bits = uint32_t(u(0, 4));
    float f;
    std::memcpy(&f, &bits, 4);
    return f;
  }
  case 12: {
    uint64_t bits = u(0, 8);
    double f;
    std::memcpy(&f, &bits, 8);
    return f;
  }
  default:
    return NAN;
  }
}

// Subclass to reach the protected black-level normalisation LibRaw itself uses.
class AigleRaw : public LibRaw {
public:
  AigleRaw() : LibRaw(LIBRAW_OPTIONS_NONE) { set_exifparser_handler(&exif_trampoline, this); }

  // Restore colour data from the unpacked raw and fold black levels the way
  // dcraw_process() does. Afterwards C.cblack[c] already includes the common C.black, so
  //   black(row, col) = C.cblack[fcol(row, col)] + C.cblack[6 + pattern(row, col)]
  // and the white range is C.maximum - C.black.
  void prepare_black() {
    raw2image_start();
    adjust_bl();
  }

  aigle_cancel_fn cancel_fn = nullptr;
  void *cancel_user = nullptr;

  // EXIF fields LibRaw does not keep, captured from its EXIF parser callback.
  struct Exif {
    char datetime[32] = {};
    char offset[16] = {};
    char copyright[128] = {};
    float bias = NAN;
    int program = -1, metering = -1, flash = -1, mode = -1, wb = -1;
  } exif;

  static void exif_trampoline(void *ctx, int tag, int type, int len, unsigned int ord, void *ifp,
                              INT64) {
    try {
      static_cast<AigleRaw *>(ctx)->on_exif(tag, type, len, short(ord), ifp);
    } catch (...) {
    }
  }

  // `tag` carries LibRaw's IFD marker in the high bits: 0 for the EXIF IFD, (ifd + 1) << 20
  // for TIFF IFDs; 0x30000.. for Panasonic and maker-note sub-IFDs, which reuse tag numbers.
  void on_exif(int tag, int type, int len, short order, void *ifp) {
    const int t = tag & 0xffff;
    const bool exif_ifd = (tag >> 16) == 0;
    const bool tiff_ifd = (tag & 0xf0000) == 0;
    auto *s = static_cast<LibRaw_abstract_datastream *>(ifp);
    auto ascii = [&](char *dst, size_t n) {
      if (dst[0] || len <= 0)
        return;
      size_t k = std::min(n - 1, size_t(len));
      int got = s->read(dst, 1, k);
      dst[got > 0 ? got : 0] = 0;
    };
    // First value of a numeric tag (SHORT, LONG, (S)RATIONAL, ...).
    auto number = [&](double &v) {
      unsigned char b[8] = {};
      if (len < 1 || s->read(b, 1, 8) < 2)
        return false;
      v = tiff_real(order, type, b);
      return std::isfinite(v);
    };
    auto code = [&](int &dst) {
      double v;
      if (dst < 0 && number(v) && v >= 0 && v < 65536)
        dst = int(v);
    };
    if (t == 0x8298 && tiff_ifd)
      ascii(exif.copyright, sizeof(exif.copyright));
    if (!exif_ifd)
      return;
    switch (t) {
    case 0x9003:
      ascii(exif.datetime, sizeof(exif.datetime));
      break;
    case 0x9011:
      ascii(exif.offset, sizeof(exif.offset));
      break;
    case 0x9204: {
      double v;
      if (std::isnan(exif.bias) && number(v) && std::fabs(v) < 100)
        exif.bias = float(v);
      break;
    }
    case 0x8822:
      code(exif.program);
      break;
    case 0x9207:
      code(exif.metering);
      break;
    case 0x9209:
      code(exif.flash);
      break;
    case 0xa402:
      code(exif.mode);
      break;
    case 0xa403:
      code(exif.wb);
      break;
    }
  }
};

int progress_cb(void *data, enum LibRaw_progress, int, int) {
  AigleRaw *r = static_cast<AigleRaw *>(data);
  return (r->cancel_fn && r->cancel_fn(r->cancel_user)) ? 1 : 0;
}

void copy_str(char *dst, size_t n, const char *src) {
  std::strncpy(dst, src ? src : "", n - 1);
  dst[n - 1] = 0;
}

AigleRaw *R(void *h) { return static_cast<AigleRaw *>(h); }

} // namespace

extern "C" {

void *aigle_raw_new(void) {
  try {
    return new (std::nothrow) AigleRaw();
  } catch (...) {
    return nullptr;
  }
}

void aigle_raw_free(void *h) {
  try {
    delete R(h);
  } catch (...) {
  }
}

int aigle_raw_open(void *h, const char *path) {
  try {
    R(h)->exif = AigleRaw::Exif{};
    return R(h)->open_file(path);
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

void aigle_raw_get_size(void *h, int *width, int *height, int *raw_width, int *raw_height) {
  libraw_data_t &d = R(h)->imgdata;
  *width = d.sizes.width;
  *height = d.sizes.height;
  *raw_width = d.sizes.raw_width;
  *raw_height = d.sizes.raw_height;
}

int aigle_raw_unpack(void *h, aigle_cancel_fn cancel, void *user) {
  try {
    AigleRaw *r = R(h);
    r->cancel_fn = cancel;
    r->cancel_user = user;
    r->set_progress_handler(progress_cb, r);
    int rc = r->unpack();
    r->set_progress_handler(nullptr, nullptr);
    r->cancel_fn = nullptr;
    return rc;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

const char *aigle_raw_strerror(int code) { return libraw_strerror(code); }

int aigle_raw_get_meta(void *h, aigle_raw_meta *out) {
  try {
    AigleRaw *r = R(h);
    r->prepare_black();
    libraw_data_t &d = r->imgdata;
    std::memset(out, 0, sizeof(*out));
    out->width = d.sizes.width;
    out->height = d.sizes.height;
    out->flip = d.sizes.flip;
    out->colors = d.idata.colors;
    out->filters = d.idata.filters;
    std::memcpy(out->cam_mul, d.color.cam_mul, sizeof(out->cam_mul));
    std::memcpy(out->pre_mul, d.color.pre_mul, sizeof(out->pre_mul));
    std::memcpy(out->cam_xyz, d.color.cam_xyz, sizeof(out->cam_xyz));
    std::memcpy(out->rgb_cam, d.color.rgb_cam, sizeof(out->rgb_cam));
    out->iso = d.other.iso_speed;
    out->shutter = d.other.shutter;
    out->aperture = d.other.aperture;
    out->focal_len = d.other.focal_len;
    out->baseline_exposure = d.color.dng_levels.baseline_exposure;
    for (int i = 0; i < 4; i++)
      out->black[i] = float(d.color.cblack[i]);
    out->white = float(d.color.maximum);
    out->black_pattern_h = d.color.cblack[4];
    out->black_pattern_w = d.color.cblack[5];
    {
      const libraw_lensinfo_t &L = d.lens;
      const char *lm = L.Lens[0] ? L.Lens : L.makernotes.Lens;
      copy_str(out->lens_model, sizeof(out->lens_model), lm);
      copy_str(out->lens_make, sizeof(out->lens_make), L.LensMake);
      out->focal_35mm = L.FocalLengthIn35mmFormat ? float(L.FocalLengthIn35mmFormat)
                                                  : L.makernotes.FocalLengthIn35mmFormat;
      out->lens_min_focal = L.MinFocal > 0 ? L.MinFocal : L.makernotes.MinFocal;
      out->lens_max_focal = L.MaxFocal > 0 ? L.MaxFocal : L.makernotes.MaxFocal;
    }
    copy_str(out->make, sizeof(out->make), d.idata.make);
    copy_str(out->model, sizeof(out->model), d.idata.model);

    {
      const AigleRaw::Exif &x = r->exif;
      copy_str(out->datetime_original, sizeof(out->datetime_original), x.datetime);
      if (!out->datetime_original[0] && d.other.timestamp > 0) {
        std::tm tm{};
        time_t ts = d.other.timestamp;
        if (localtime_r(&ts, &tm))
          std::strftime(out->datetime_original, sizeof(out->datetime_original),
                        "%Y:%m:%d %H:%M:%S", &tm);
      }
      copy_str(out->offset_time, sizeof(out->offset_time), x.offset);
      copy_str(out->copyright, sizeof(out->copyright), x.copyright);
      out->exposure_bias = x.bias;
      out->exposure_program = x.program;
      out->metering_mode = x.metering;
      out->flash = x.flash;
      out->exposure_mode = x.mode;
      out->white_balance = x.wb;
      out->max_aperture = d.lens.EXIF_MaxAp > 0 ? d.lens.EXIF_MaxAp : NAN;
      copy_str(out->body_serial, sizeof(out->body_serial), d.shootinginfo.BodySerial);
      copy_str(out->lens_serial, sizeof(out->lens_serial), d.lens.LensSerial);
      copy_str(out->firmware, sizeof(out->firmware), d.makernotes.common.firmware);
      copy_str(out->artist, sizeof(out->artist), d.other.artist);
      const libraw_gps_info_t &g = d.other.parsed_gps;
      if (g.gpsparsed) {
        auto deg = [](const float *dms) {
          return double(dms[0]) + double(dms[1]) / 60.0 + double(dms[2]) / 3600.0;
        };
        out->gps_valid = 1;
        out->gps_lat = deg(g.latitude) * (g.latref == 'S' ? -1 : 1);
        out->gps_lon = deg(g.longitude) * (g.longref == 'W' ? -1 : 1);
        out->gps_alt = g.altitude != 0 ? g.altitude * (g.altref == 1 ? -1 : 1) : NAN;
      } else {
        out->gps_alt = NAN;
      }
    }
    const char *why = nullptr;
    if (d.rawdata.raw_image && d.idata.filters) {
      if (r->get_internal_data_pointer()->internal_output_params.fuji_width)
        why = "Fujifilm SuperCCD layout";
      else if (d.idata.filters == 1)
        why = "Leaf 16x16 CFA";
      else if (d.idata.colors != 3 || std::strncmp(d.idata.cdesc, "RGBG", 4) != 0)
        why = "non-RGB colour filter array";
      else {
        out->kind = d.idata.filters == 9 ? AIGLE_RAW_XTRANS : AIGLE_RAW_BAYER;
        for (int row = 0; row < 6; row++)
          for (int col = 0; col < 6; col++) {
            int c = r->fcol(row, col);
            out->cfa[row][col] = c == 3 ? 1 : c;
          }
      }
    } else if (d.rawdata.raw_image && d.idata.colors == 1) {
      why = "monochrome sensor";
    } else if (d.rawdata.color3_image || d.rawdata.color4_image) {
      out->kind = AIGLE_RAW_RGB;
    } else if (d.rawdata.float_image || d.rawdata.float3_image || d.rawdata.float4_image) {
      why = "floating-point raw data";
    } else {
      why = "unrecognised raw data layout";
    }
    if (why) {
      out->kind = AIGLE_RAW_UNSUPPORTED;
      copy_str(out->unsupported_reason, sizeof(out->unsupported_reason), why);
    }
    return 0;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

int aigle_raw_copy_cfa(void *h, const float *white, float *out, uint8_t *clip,
                       uint64_t *clipped) {
  try {
    AigleRaw *r = R(h);
    r->prepare_black();
    libraw_data_t &d = r->imgdata;
    if (!d.rawdata.raw_image || !d.idata.filters)
      return LIBRAW_OUT_OF_ORDER_CALL;
    const libraw_colordata_t &C = d.color;
    const int w = d.sizes.width, hgt = d.sizes.height;
    const int top = d.sizes.top_margin, left = d.sizes.left_margin;
    const size_t pitch = d.sizes.raw_pitch / 2;
    const unsigned pw = C.cblack[5], ph = C.cblack[4];
    // Per colour index white range: `white[c]` (absolute raw units, capped at LibRaw's
    // maximum) when given, else the nominal maximum. Data is clamped to it and normalised
    // per channel; clipping is detected just below it.
    float range[4], inv[4], clip_at[4];
    for (int c = 0; c < 4; c++) {
      float wl = float(C.maximum);
      if (white && white[c] > 0 && white[c] < wl)
        wl = white[c];
      range[c] = wl - float(C.black);
      if (range[c] <= 0)
        return LIBRAW_UNSPECIFIED_ERROR;
      inv[c] = 1.0f / range[c];
      clip_at[c] = 0.995f * range[c];
    }
    uint64_t n = 0;
    for (int row = 0; row < hgt; row++) {
      const ushort *src = d.rawdata.raw_image + size_t(row + top) * pitch + left;
      const int maxcol = (left + w <= d.sizes.raw_width) ? w : d.sizes.raw_width - left;
      bool valid_row = row + top < d.sizes.raw_height;
      for (int col = 0; col < w; col++) {
        float v = 0;
        int cc = r->fcol(row, col) & 3;
        if (valid_row && col < maxcol) {
          float black = float(C.cblack[cc]);
          if (pw && ph)
            black += float(C.cblack[6 + (row % ph) * pw + (col % pw)]);
          v = std::min(float(src[col]) - black, range[cc]);
        }
        size_t i = size_t(row) * w + col;
        out[i] = v * inv[cc];
        bool c = v >= clip_at[cc];
        if (clip)
          clip[i] = c ? 1 : 0;
        n += c;
      }
    }
    if (clipped)
      *clipped = n;
    return 0;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

int aigle_raw_copy_rgb(void *h, float *out) {
  try {
    AigleRaw *r = R(h);
    r->prepare_black();
    libraw_data_t &d = r->imgdata;
    const libraw_colordata_t &C = d.color;
    const int w = d.sizes.width, hgt = d.sizes.height;
    const int top = d.sizes.top_margin, left = d.sizes.left_margin;
    const float range = float(C.maximum) - float(C.black);
    if (range <= 0)
      return LIBRAW_UNSPECIFIED_ERROR;
    for (int row = 0; row < hgt; row++) {
      for (int col = 0; col < w; col++) {
        const ushort *px;
        if (d.rawdata.color3_image)
          px = d.rawdata.color3_image[size_t(row + top) * (d.sizes.raw_pitch / 6) + col + left];
        else if (d.rawdata.color4_image)
          px = d.rawdata.color4_image[size_t(row + top) * (d.sizes.raw_pitch / 8) + col + left];
        else
          return LIBRAW_OUT_OF_ORDER_CALL;
        for (int c = 0; c < 3; c++) {
          float black = float(C.cblack[c]);
          out[(size_t(row) * w + col) * 3 + c] = (float(px[c]) - black) / range;
        }
      }
    }
    return 0;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

int aigle_raw_get_thumb(void *h, aigle_raw_thumb *out) {
  try {
    std::memset(out, 0, sizeof(*out));
    AigleRaw *r = R(h);
    int rc = r->unpack_thumb();
    if (rc != LIBRAW_SUCCESS)
      return rc;
    int err = 0;
    libraw_processed_image_t *img = r->dcraw_make_mem_thumb(&err);
    if (!img)
      return err ? err : LIBRAW_NO_THUMBNAIL;
    if (img->type == LIBRAW_IMAGE_JPEG)
      out->kind = 1;
    else if (img->type == LIBRAW_IMAGE_BITMAP && img->colors == 3 && img->bits == 8)
      out->kind = 2;
    else {
      LibRaw::dcraw_clear_mem(img);
      return LIBRAW_NO_THUMBNAIL;
    }
    out->width = img->width;
    out->height = img->height;
    out->data = img->data;
    out->len = img->data_size;
    out->opaque = img;
    return 0;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

void aigle_raw_thumb_free(aigle_raw_thumb *t) {
  if (t && t->opaque) {
    LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t *>(t->opaque));
    t->opaque = nullptr;
    t->data = nullptr;
  }
}

} // extern "C"
