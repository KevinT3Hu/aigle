// aigle shim over libvips (+ libexif for written EXIF): a narrow C ABI for the engine's
// image I/O (doc 02 §15, doc 04 §4, §6).
//
// Loading: any format libvips reads (JPEG, PNG, TIFF, WebP, HEIF/AVIF, ...), autorotated,
// Alpha dropped, converted through its embedded ICC profile (sRGB if none) by lcms2 to
// float XYZ, which libvips adapts from the D50 PCS to D65 (white = (95.05, 100, 108.9)).
//
// Saving: the image is generated on demand by a callback that fills rectangles (libvips
// asks for strips, from its worker threads), optionally resized in linear light, tagged
// with an ICC profile and EXIF, and written to a file or a buffer. The whole image is never
// resident.

#include <math.h>
#include <string.h>

#include <libexif/exif-data.h>
#include <libexif/exif-entry.h>
#include <libexif/exif-tag.h>
#include <libexif/exif-utils.h>
#include <vips/vips.h>

static char last_error[1024];

static int fail(void) {
  char *e = vips_error_buffer_copy();
  g_strlcpy(last_error, e ? e : "unknown libvips error", sizeof(last_error));
  g_free(e);
  return -1;
}

const char *aigle_vips_last_error(void) { return last_error; }

int aigle_vips_init(void) {
  static int done = 0, rc = 0;
  if (!done) {
    done = 1;
    if (VIPS_INIT("aigle")) {
      rc = fail();
    } else {
      // Operations are not reused across images; the cache would only hold pixels.
      vips_cache_set_max(0);
    }
  }
  return rc;
}

typedef struct {
  int width, height; // after autorotation
  int orientation;   // EXIF orientation of the file (1 if none)
  int bands;
  int has_icc;
} aigle_vips_header;

void *aigle_vips_open(const char *path, aigle_vips_header *h) {
  VipsImage *im = vips_image_new_from_file(path, NULL);
  if (!im) {
    fail();
    return NULL;
  }
  int o = 1;
  if (vips_image_get_typeof(im, VIPS_META_ORIENTATION))
    vips_image_get_int(im, VIPS_META_ORIENTATION, &o);
  if (o < 1 || o > 8)
    o = 1;
  h->orientation = o;
  int swap = o >= 5;
  h->width = swap ? im->Ysize : im->Xsize;
  h->height = swap ? im->Xsize : im->Ysize;
  h->bands = im->Bands;
  h->has_icc = vips_image_get_typeof(im, VIPS_META_ICC_NAME) != 0;
  return im;
}

void aigle_vips_unref(void *im) {
  if (im)
    g_object_unref(im);
}

// Metadata string `name` (e.g. "exif-ifd0-Make") into `buf`; 0 if present.
int aigle_vips_get_string(void *im, const char *name, char *buf, int len) {
  VipsImage *v = im;
  if (!vips_image_get_typeof(v, name))
    return -1;
  char *s = NULL;
  if (vips_image_get_as_string(v, name, &s))
    return fail();
  g_strlcpy(buf, s, len);
  g_free(s);
  return 0;
}

// Decode into `out` (width × height × 3 float XYZ, D65, Y of white = 100). `cancel(user)`
// is polled between scanline blocks; returns 1 when cancelled.
int aigle_vips_decode_xyz(void *img, float *out, int (*cancel)(void *), void *user) {
  VipsImage *base = img;
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 6);
  VipsImage *in = base;
  int rc = 0;
  if (vips_autorot(in, &t[0], NULL)) {
    rc = fail();
    goto done;
  }
  in = t[0];
  if (vips_image_hasalpha(in)) {
    if (vips_extract_band(in, &t[1], 0, "n", in->Bands - 1, NULL)) {
      rc = fail();
      goto done;
    }
    in = t[1];
  }
  if (in->Type == VIPS_INTERPRETATION_scRGB) {
    // Linear sRGB float (e.g. float TIFF): to XYZ (D65) through libvips' own matrix.
    if (vips_colourspace(in, &t[2], VIPS_INTERPRETATION_XYZ, NULL)) {
      rc = fail();
      goto done;
    }
    in = t[2];
  } else {
    // 1-band images without a profile are taken as sRGB grey.
    if (in->Bands < 3 && !vips_image_get_typeof(in, VIPS_META_ICC_NAME)) {
      if (vips_colourspace(in, &t[3], VIPS_INTERPRETATION_sRGB, NULL)) {
        rc = fail();
        goto done;
      }
      in = t[3];
    }
    if (vips_icc_import(in, &t[4], "embedded", TRUE, "input_profile", "srgb", "pcs", VIPS_PCS_XYZ,
                        "intent", VIPS_INTENT_RELATIVE, NULL)) {
      rc = fail();
      goto done;
    }
    in = t[4];
  }
  if (vips_cast(in, &t[5], VIPS_FORMAT_FLOAT, NULL)) {
    rc = fail();
    goto done;
  }
  in = t[5];
  {
    VipsRegion *reg = vips_region_new(in);
    int w = in->Xsize, h = in->Ysize;
    for (int y = 0; y < h && !rc; y += 64) {
      if (cancel && cancel(user)) {
        rc = 1;
        break;
      }
      VipsRect r = {0, y, w, VIPS_MIN(64, h - y)};
      if (vips_region_prepare(reg, &r)) {
        rc = fail();
        break;
      }
      for (int j = 0; j < r.height; j++)
        memcpy(out + (size_t)(y + j) * w * 3, VIPS_REGION_ADDR(reg, 0, y + j),
               (size_t)w * 3 * sizeof(float));
    }
    g_object_unref(reg);
  }
done:
  g_object_unref(ctx);
  return rc;
}

// ---------------------------------------------------------------------------------------------
// Saving

typedef int (*aigle_vips_fill_fn)(void *user, int x, int y, int w, int h, void *out, size_t stride);

typedef struct {
  aigle_vips_fill_fn fill;
  void *user;
} gen_ctx;

static int gen(VipsRegion *out, void *seq, void *a, void *b, gboolean *stop) {
  gen_ctx *c = a;
  VipsRect *r = &out->valid;
  if (c->fill(c->user, r->left, r->top, r->width, r->height, VIPS_REGION_ADDR(out, r->left, r->top),
              VIPS_REGION_LSKIP(out)))
    return -1;
  return 0;
}

typedef struct {
  const char *make, *model, *lens_make, *lens_model, *software, *datetime;
  const char *artist, *copyright;
  double exposure_time, f_number, focal_length, iso; // 0 = absent
  double exposure_bias;                               // NaN = absent
} aigle_vips_exif;

static void exif_srational(ExifData *ed, ExifIfd ifd, ExifTag tag, double v) {
  if (isnan(v))
    return;
  ExifEntry *e = exif_entry_new();
  ExifMem *mem = exif_mem_new_default();
  e->data = exif_mem_alloc(mem, 8);
  e->size = 8;
  e->components = 1;
  e->format = EXIF_FORMAT_SRATIONAL;
  e->tag = tag;
  ExifSRational r = {(ExifSLong)lround(v * 100), 100};
  exif_set_srational(e->data, exif_data_get_byte_order(ed), r);
  exif_content_add_entry(ed->ifd[ifd], e);
  exif_entry_unref(e);
  exif_mem_unref(mem);
}

static void exif_ascii(ExifData *ed, ExifIfd ifd, ExifTag tag, const char *s) {
  if (!s || !*s)
    return;
  ExifEntry *e = exif_entry_new();
  ExifMem *mem = exif_mem_new_default();
  size_t n = strlen(s) + 1;
  e->data = exif_mem_alloc(mem, n);
  memcpy(e->data, s, n);
  e->size = n;
  e->components = n;
  e->format = EXIF_FORMAT_ASCII;
  e->tag = tag;
  exif_content_add_entry(ed->ifd[ifd], e);
  exif_entry_unref(e);
  exif_mem_unref(mem);
}

static void exif_rational(ExifData *ed, ExifIfd ifd, ExifTag tag, double v) {
  if (v <= 0)
    return;
  ExifEntry *e = exif_entry_new();
  ExifMem *mem = exif_mem_new_default();
  e->data = exif_mem_alloc(mem, 8);
  e->size = 8;
  e->components = 1;
  e->format = EXIF_FORMAT_RATIONAL;
  e->tag = tag;
  ExifRational r;
  if (v < 1) {
    r.numerator = 1;
    r.denominator = (ExifLong)(1.0 / v + 0.5);
  } else {
    r.numerator = (ExifLong)(v * 100 + 0.5);
    r.denominator = 100;
  }
  exif_set_rational(e->data, exif_data_get_byte_order(ed), r);
  exif_content_add_entry(ed->ifd[ifd], e);
  exif_entry_unref(e);
  exif_mem_unref(mem);
}

static void exif_short(ExifData *ed, ExifIfd ifd, ExifTag tag, int v) {
  ExifEntry *e = exif_entry_new();
  ExifMem *mem = exif_mem_new_default();
  e->data = exif_mem_alloc(mem, 2);
  e->size = 2;
  e->components = 1;
  e->format = EXIF_FORMAT_SHORT;
  e->tag = tag;
  exif_set_short(e->data, exif_data_get_byte_order(ed), (ExifShort)v);
  exif_content_add_entry(ed->ifd[ifd], e);
  exif_entry_unref(e);
  exif_mem_unref(mem);
}

static int attach_exif(VipsImage *im, const aigle_vips_exif *x) {
  ExifData *ed = exif_data_new();
  exif_data_set_byte_order(ed, EXIF_BYTE_ORDER_INTEL);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_MAKE, x->make);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_MODEL, x->model);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_SOFTWARE, x->software);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_DATE_TIME, x->datetime);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_ARTIST, x->artist);
  exif_ascii(ed, EXIF_IFD_0, EXIF_TAG_COPYRIGHT, x->copyright);
  exif_short(ed, EXIF_IFD_0, EXIF_TAG_ORIENTATION, 1);
  exif_srational(ed, EXIF_IFD_EXIF, EXIF_TAG_EXPOSURE_BIAS_VALUE, x->exposure_bias);
  exif_ascii(ed, EXIF_IFD_EXIF, EXIF_TAG_DATE_TIME_ORIGINAL, x->datetime);
  exif_rational(ed, EXIF_IFD_EXIF, EXIF_TAG_EXPOSURE_TIME, x->exposure_time);
  exif_rational(ed, EXIF_IFD_EXIF, EXIF_TAG_FNUMBER, x->f_number);
  exif_rational(ed, EXIF_IFD_EXIF, EXIF_TAG_FOCAL_LENGTH, x->focal_length);
  if (x->iso > 0)
    exif_short(ed, EXIF_IFD_EXIF, EXIF_TAG_ISO_SPEED_RATINGS, (int)(x->iso + 0.5));
  exif_ascii(ed, EXIF_IFD_EXIF, (ExifTag)0xa433, x->lens_make);  // LensMake
  exif_ascii(ed, EXIF_IFD_EXIF, (ExifTag)0xa434, x->lens_model); // LensModel
  unsigned char *data = NULL;
  unsigned int size = 0;
  exif_data_save_data(ed, &data, &size);
  exif_data_unref(ed);
  if (!data)
    return 0;
  vips_image_set_blob_copy(im, VIPS_META_EXIF_NAME, data, size);
  free(data);
  return 0;
}

typedef struct {
  int width, height;
  int sixteen_bit;      // ushort samples, else uchar
  int out_width, out_height; // resize target (linear-light Lanczos-3), or the source size
  const char *icc;      // libvips built-in profile name ("srgb", "p3") or NULL
  const aigle_vips_exif *exif; // NULL = none
  const char *suffix;   // saver and options, e.g. ".jpg[Q=90,strip=false]"
  double sharpen_sigma, sharpen_amount; // unsharp mask on lightness after resizing; 0 = off
  const void *icc_data; // ICC profile to embed instead of `icc`, or NULL
  size_t icc_len;
  double resize_gamma;  // > 0: resize linearised by this power instead of the sRGB curve
  // Watermark blended over the output after resizing and sharpening: `ov_w × ov_h` RGB
  // samples in the output's format and an 8-bit alpha, at (ov_x, ov_y). NULL = none.
  const void *ov_rgb;
  const unsigned char *ov_alpha;
  int ov_x, ov_y, ov_w, ov_h;
} aigle_vips_save_opts;

// `x` in [0, ∞) with negative ringing folded to 0: (x + |x|) / 2.
static int clip_negative(VipsImage *in, VipsImage **out, VipsObject *ctx) {
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 2);
  if (vips_abs(in, &t[0], NULL) || vips_add(in, t[0], &t[1], NULL) ||
      vips_linear1(t[1], out, 0.5, 0.0, NULL))
    return -1;
  return 0;
}

// Resize `in` (uchar or ushort, encoded with a power-law curve of `gamma`) in linear light.
static int resize_power(VipsImage *in, VipsImage **out, double hs, double vs, double gamma,
                        int sixteen, VipsObject *ctx) {
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 6);
  double max = sixteen ? 65535.0 : 255.0;
  if (vips_linear1(in, &t[0], 1.0 / max, 0.0, NULL) ||
      vips_math2_const1(t[0], &t[1], VIPS_OPERATION_MATH2_POW, gamma, NULL) ||
      vips_resize(t[1], &t[2], hs, "vscale", vs, "kernel", VIPS_KERNEL_LANCZOS3, NULL) ||
      clip_negative(t[2], &t[3], ctx) ||
      vips_math2_const1(t[3], &t[4], VIPS_OPERATION_MATH2_POW, 1.0 / gamma, NULL) ||
      vips_linear1(t[4], &t[5], max, 0.5, NULL) ||
      vips_cast(t[5], out, sixteen ? VIPS_FORMAT_USHORT : VIPS_FORMAT_UCHAR, NULL))
    return -1;
  return 0;
}

// Build the output image (a pipeline over `fill`) into `*out`; objects owned by `ctx`.
static int build(const aigle_vips_save_opts *o, aigle_vips_fill_fn fill, void *user, VipsObject *ctx,
                 VipsImage **out) {
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 8);
  gen_ctx *c = vips_malloc(ctx, sizeof(gen_ctx));
  c->fill = fill;
  c->user = user;
  t[0] = vips_image_new();
  VipsBandFormat fmt = o->sixteen_bit ? VIPS_FORMAT_USHORT : VIPS_FORMAT_UCHAR;
  VipsInterpretation interp = o->sixteen_bit ? VIPS_INTERPRETATION_RGB16 : VIPS_INTERPRETATION_sRGB;
  vips_image_init_fields(t[0], o->width, o->height, 3, fmt, VIPS_CODING_NONE, interp, 1.0, 1.0);
  if (vips_image_pipelinev(t[0], VIPS_DEMAND_STYLE_THINSTRIP, NULL) ||
      vips_image_generate(t[0], NULL, gen, NULL, c, NULL))
    return fail();
  VipsImage *im = t[0];
  if (o->out_width != o->width || o->out_height != o->height) {
    double hs = (double)o->out_width / o->width, vs = (double)o->out_height / o->height;
    if (o->resize_gamma > 0) {
      if (resize_power(im, &t[3], hs, vs, o->resize_gamma, o->sixteen_bit, ctx))
        return fail();
      if (vips_copy(t[3], &t[7], "interpretation", interp, NULL))
        return fail();
      im = t[7];
    } else {
      // Resize in linear light; the sRGB transfer is shared by sRGB and Display P3.
      if (vips_colourspace(im, &t[1], VIPS_INTERPRETATION_scRGB, NULL) ||
          vips_resize(t[1], &t[2], hs, "vscale", vs, "kernel", VIPS_KERNEL_LANCZOS3, NULL) ||
          vips_colourspace(t[2], &t[3], interp, NULL))
        return fail();
      im = t[3];
    }
  }
  if (o->sharpen_sigma > 0) {
    // Flat areas (m1 = 0) stay untouched; sharpening works on L and returns to `interp`.
    if (vips_sharpen(im, &t[5], "sigma", o->sharpen_sigma, "m1", 0.0, "m2", o->sharpen_amount,
                     NULL) ||
        vips_colourspace(t[5], &t[6], interp, NULL))
      return fail();
    im = t[6];
  }
  if (o->ov_rgb && o->ov_alpha && o->ov_w > 0 && o->ov_h > 0) {
    VipsImage **w = (VipsImage **)vips_object_local_array(ctx, 6);
    size_t bps = o->sixteen_bit ? 2 : 1;
    w[0] = vips_image_new_from_memory(o->ov_rgb, (size_t)o->ov_w * o->ov_h * 3 * bps, o->ov_w,
                                      o->ov_h, 3, fmt);
    w[1] = vips_image_new_from_memory(o->ov_alpha, (size_t)o->ov_w * o->ov_h, o->ov_w, o->ov_h,
                                      1, VIPS_FORMAT_UCHAR);
    if (!w[0] || !w[1] ||
        vips_embed(w[0], &w[2], o->ov_x, o->ov_y, im->Xsize, im->Ysize, NULL) ||
        vips_embed(w[1], &w[3], o->ov_x, o->ov_y, im->Xsize, im->Ysize, NULL) ||
        vips_ifthenelse(w[3], w[2], im, &w[4], "blend", TRUE, NULL) ||
        vips_copy(w[4], &w[5], "interpretation", interp, NULL))
      return fail();
    im = w[5];
  }
  if (vips_copy(im, &t[4], NULL))
    return fail();
  im = t[4];
  if (o->icc_data && o->icc_len) {
    vips_image_set_blob_copy(im, VIPS_META_ICC_NAME, o->icc_data, o->icc_len);
  } else if (o->icc) {
    VipsBlob *blob = NULL;
    if (vips_profile_load(o->icc, &blob, NULL))
      return fail();
    if (blob) {
      size_t n;
      const void *d = vips_blob_get(blob, &n);
      vips_image_set_blob_copy(im, VIPS_META_ICC_NAME, d, n);
      vips_area_unref(VIPS_AREA(blob));
    }
  }
  if (o->exif)
    attach_exif(im, o->exif);
  *out = im;
  return 0;
}

int aigle_vips_save_file(const aigle_vips_save_opts *o, aigle_vips_fill_fn fill, void *user,
                         const char *path) {
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage *im = NULL;
  int rc = build(o, fill, user, ctx, &im);
  if (!rc) {
    char *name = g_strconcat(path, o->suffix, NULL);
    if (vips_image_write_to_file(im, name, NULL))
      rc = fail();
    g_free(name);
  }
  g_object_unref(ctx);
  return rc;
}

int aigle_vips_save_buffer(const aigle_vips_save_opts *o, aigle_vips_fill_fn fill, void *user, void **buf,
                           size_t *len) {
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage *im = NULL;
  int rc = build(o, fill, user, ctx, &im);
  if (!rc && vips_image_write_to_buffer(im, o->suffix, buf, len, NULL))
    rc = fail();
  g_object_unref(ctx);
  return rc;
}

void aigle_vips_free(void *p) { g_free(p); }

// Decode an encoded image in memory to `w × h` 8-bit sRGB RGB (thumbnails from the disk
// cache). `*out` is allocated with g_malloc.
int aigle_vips_decode_rgb8(const void *data, size_t len, int *w, int *h, unsigned char **out) {
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 3);
  int rc = 0;
  t[0] = vips_image_new_from_buffer(data, len, "", NULL);
  if (!t[0] || vips_colourspace(t[0], &t[1], VIPS_INTERPRETATION_sRGB, NULL) ||
      vips_extract_band(t[1], &t[2], 0, "n", 3, NULL)) {
    rc = fail();
  } else {
    size_t n;
    *out = vips_image_write_to_memory(t[2], &n);
    if (!*out) {
      rc = fail();
    } else {
      *w = t[2]->Xsize;
      *h = t[2]->Ysize;
    }
  }
  g_object_unref(ctx);
  return rc;
}


// Watermark sources. Both return `*w × *h` pixels allocated with g_malloc.

// One line of `text` (Pango markup is not interpreted) in `font` (a Pango description
// without size), scaled to exactly `height` px: an 8-bit coverage mask.
int aigle_vips_text_mask(const char *text, const char *font, int height, int *w, int *h,
                         unsigned char **out) {
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 3);
  int rc = 0;
  char *escaped = g_markup_escape_text(text, -1);
  char *desc = g_strdup_printf("%s %d", font, height > 1 ? height : 1);
  if (vips_text(&t[0], escaped, "font", desc, "dpi", 72, NULL)) {
    rc = fail();
  } else if (t[0]->Ysize < 1 || t[0]->Xsize < 1) {
    g_strlcpy(last_error, "watermark text renders empty", sizeof(last_error));
    rc = -1;
  } else {
    double s = (double)height / t[0]->Ysize;
    if (vips_resize(t[0], &t[1], s, NULL) || vips_cast(t[1], &t[2], VIPS_FORMAT_UCHAR, NULL)) {
      rc = fail();
    } else {
      size_t n;
      *out = vips_image_write_to_memory(t[2], &n);
      if (!*out) {
        rc = fail();
      } else {
        *w = t[2]->Xsize;
        *h = t[2]->Ysize;
      }
    }
  }
  g_free(desc);
  g_free(escaped);
  g_object_unref(ctx);
  return rc;
}

// The image at `path`, converted to sRGB through its profile, scaled to `height` px tall
// (vector formats are rendered at that size), as RGBA8 (opaque if it has no alpha).
int aigle_vips_load_rgba8(const char *path, int height, int *w, int *h, unsigned char **out) {
  VipsObject *ctx = VIPS_OBJECT(vips_image_new());
  VipsImage **t = (VipsImage **)vips_object_local_array(ctx, 4);
  int rc = 0;
  if (vips_thumbnail(path, &t[0], VIPS_MAX_COORD, "height", height > 1 ? height : 1, NULL) ||
      vips_colourspace(t[0], &t[1], VIPS_INTERPRETATION_sRGB, NULL) ||
      vips_cast(t[1], &t[2], VIPS_FORMAT_UCHAR, NULL)) {
    rc = fail();
  } else {
    VipsImage *im = t[2];
    if (im->Bands == 3) {
      if (vips_bandjoin_const1(im, &t[3], 255, NULL)) {
        rc = fail();
        goto done;
      }
      im = t[3];
    } else if (im->Bands != 4) {
      g_strlcpy(last_error, "watermark image is not RGB", sizeof(last_error));
      rc = -1;
      goto done;
    }
    size_t n;
    *out = vips_image_write_to_memory(im, &n);
    if (!*out) {
      rc = fail();
    } else {
      *w = im->Xsize;
      *h = im->Ysize;
    }
  }
done:
  g_object_unref(ctx);
  return rc;
}
