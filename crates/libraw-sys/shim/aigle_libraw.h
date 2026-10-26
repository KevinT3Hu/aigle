/* aigle shim over LibRaw: a narrow, exception-free C ABI exposing exactly what the A tier
 * needs (doc 02 §3, A1/A2). Every entry point catches all C++ exceptions. */
#ifndef AIGLE_LIBRAW_H
#define AIGLE_LIBRAW_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum aigle_raw_kind {
  AIGLE_RAW_BAYER = 0,
  AIGLE_RAW_XTRANS = 1,
  /* Already demosaiced linear data (linear DNG, sRAW). */
  AIGLE_RAW_RGB = 2,
  AIGLE_RAW_UNSUPPORTED = 3,
};

typedef struct aigle_raw_meta {
  /* Visible area, sensor orientation. */
  int32_t width, height;
  /* LibRaw/dcraw flip: bit 2 = transpose, bit 1 = flip vertical, bit 0 = flip horizontal. */
  int32_t flip;
  int32_t kind;
  int32_t colors;
  uint32_t filters;
  /* CFA colour (0 = R, 1 = G, 2 = B) at visible (row % 6, col % 6). */
  int32_t cfa[6][6];
  float cam_mul[4];
  float pre_mul[4];
  /* XYZ (D65) -> camera, rows per camera channel (dcraw/Adobe convention). */
  float cam_xyz[4][3];
  /* camera -> linear sRGB, as computed by LibRaw. */
  float rgb_cam[3][4];
  float iso, shutter, aperture, focal_len;
  /* DNG BaselineExposure in EV; -999 when absent. */
  float baseline_exposure;
  /* Black level per CFA colour index 0..3 (common + per-channel part) and white level, raw units. */
  float black[4];
  float white;
  /* Size of the extra black pattern (cblack[4] x cblack[5]), 0 if none. */
  int32_t black_pattern_h, black_pattern_w;
  /* Lens identification (EXIF / makernotes). */
  char lens_make[128];
  char lens_model[128];
  float focal_35mm;
  float lens_min_focal, lens_max_focal;
  char make[64];
  char model[64];
  char unsupported_reason[128];
  /* Capture facts (EXIF); empty strings / -1 codes / NaN floats when absent. */
  char datetime_original[32];
  char offset_time[16];
  float exposure_bias;
  int32_t exposure_program, metering_mode, flash, exposure_mode, white_balance;
  float max_aperture;
  char body_serial[64];
  char lens_serial[64];
  char firmware[128];
  char artist[64];
  char copyright[128];
  int32_t gps_valid;
  double gps_lat, gps_lon;
  float gps_alt;
} aigle_raw_meta;

typedef struct aigle_raw_thumb {
  /* 1 = JPEG bytes, 2 = 8-bit RGB bitmap (width x height x 3), 0 = none. */
  int32_t kind;
  int32_t width, height;
  const uint8_t *data;
  size_t len;
  void *opaque;
} aigle_raw_thumb;

/* Returns non-zero to request cancellation. */
typedef int (*aigle_cancel_fn)(void *user);

void *aigle_raw_new(void);
void aigle_raw_free(void *h);
/* All int-returning calls return 0 on success or a LibRaw error code (negative). */
int aigle_raw_open(void *h, const char *path);
/* Image and raw sizes after open, before unpack (for memory admission). */
void aigle_raw_get_size(void *h, int *width, int *height, int *raw_width, int *raw_height);
int aigle_raw_unpack(void *h, aigle_cancel_fn cancel, void *user);
const char *aigle_raw_strerror(int code);
int aigle_raw_get_meta(void *h, aigle_raw_meta *out);
/* Bayer/X-Trans: black-subtracted CFA normalised so that 1.0 = white level, width*height
 * floats. `white` (may be NULL) overrides the white level per CFA colour index 0..3 in
 * absolute raw units (capped at LibRaw's maximum). `clip` receives 1 for pixels at the
 * white level (may be NULL). */
int aigle_raw_copy_cfa(void *h, const float *white, float *out, uint8_t *clip,
                       uint64_t *clipped);
/* RGB kind: black-subtracted, normalised, interleaved RGB, width*height*3 floats. */
int aigle_raw_copy_rgb(void *h, float *out);
/* Embedded preview. Free with aigle_raw_thumb_free. */
int aigle_raw_get_thumb(void *h, aigle_raw_thumb *out);
void aigle_raw_thumb_free(aigle_raw_thumb *t);

#ifdef __cplusplus
}
#endif
#endif
