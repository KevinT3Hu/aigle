// aigle shim over librtprocess: plain C ABI over contiguous planes. Data is in the
// librtprocess convention, 0..65535 (doc 02 §3, A2–A4).
//
// Demosaic and CA correction poll `cancel(user)` at their progress points (every 32 tiles,
// about 1 MP) and return RP_CANCELLED when it returns non-zero; the vendored librtprocess is
// patched for that ("aigle patch" in the sources). Highlight inpainting is not cancellable.

#include <cstdint>
#include <functional>
#include <vector>

#include "librtprocess.h"

typedef int (*aigle_rp_cancel_fn)(void *user);

namespace {

struct Rows {
  std::vector<float *> p;
  Rows(float *base, int w, int h) : p(h) {
    for (int y = 0; y < h; y++)
      p[y] = base + size_t(y) * w;
  }
  float **get() { return p.data(); }
  const float *const *cget() const { return p.data(); }
};

void bayer(const int32_t cfa[2][2], unsigned out[2][2]) {
  for (int y = 0; y < 2; y++)
    for (int x = 0; x < 2; x++)
      out[y][x] = unsigned(cfa[y][x]);
}

std::function<bool(double)> progress(aigle_rp_cancel_fn cancel, void *user) {
  return [cancel, user](double) { return cancel != nullptr && cancel(user) != 0; };
}

const std::function<bool(double)> no_progress = [](double) { return false; };

} // namespace

extern "C" {

int aigle_rp_rcd(int w, int h, float *cfa, float *r, float *g, float *b, const int32_t pattern[2][2],
                 aigle_rp_cancel_fn cancel, void *user) {
  try {
    unsigned p[2][2];
    bayer(pattern, p);
    Rows in(cfa, w, h), R(r, w, h), G(g, w, h), B(b, w, h);
    // Single-threaded: OpenMP is not enabled (the engine owns the CPU, doc 05 §5).
    return rcd_demosaic(w, h, in.cget(), R.get(), G.get(), B.get(), p, progress(cancel, user), 2, false,
                        false);
  } catch (...) {
    return -1;
  }
}

int aigle_rp_amaze(int w, int h, float *cfa, float *r, float *g, float *b, const int32_t pattern[2][2],
                   aigle_rp_cancel_fn cancel, void *user) {
  try {
    unsigned p[2][2];
    bayer(pattern, p);
    Rows in(cfa, w, h), R(r, w, h), G(g, w, h), B(b, w, h);
    return amaze_demosaic(w, h, 0, 0, w, h, in.cget(), R.get(), G.get(), B.get(), p, progress(cancel, user),
                          1.0, 0, 65535.f, 65535.f);
  } catch (...) {
    return -1;
  }
}

int aigle_rp_markesteijn(int w, int h, float *cfa, float *r, float *g, float *b, const int32_t pattern[6][6],
                         const float rgb_cam[3][4], int passes, aigle_rp_cancel_fn cancel, void *user) {
  try {
    unsigned p[6][6];
    for (int y = 0; y < 6; y++)
      for (int x = 0; x < 6; x++)
        p[y][x] = unsigned(pattern[y][x]);
    Rows in(cfa, w, h), R(r, w, h), G(g, w, h), B(b, w, h);
    return markesteijn_demosaic(w, h, in.cget(), R.get(), G.get(), B.get(), p, rgb_cam,
                                progress(cancel, user), passes, false);
  } catch (...) {
    return -1;
  }
}

// In place. Auto-CA with RawTherapee's defaults (2 iterations, avoid colour shift). On
// cancellation the data is partly corrected.
int aigle_rp_ca_correct(int w, int h, float *cfa, const int32_t pattern[2][2], aigle_rp_cancel_fn cancel,
                        void *user) {
  try {
    unsigned p[2][2];
    bayer(pattern, p);
    Rows io(cfa, w, h);
    double fit[2][2][16] = {};
    return CA_correct(0, 0, w, h, true, 2, 0.0, 0.0, true, io.cget(), io.get(), p, progress(cancel, user), fit,
                      false, 65535.f, 65535.f);
  } catch (...) {
    return -1;
  }
}

int aigle_rp_hl_inpaint(int w, int h, float *r, float *g, float *b, const float chmax[3], const float clmax[3]) {
  try {
    Rows R(r, w, h), G(g, w, h), B(b, w, h);
    return HLRecovery_inpaint(w, h, R.get(), G.get(), B.get(), chmax, clmax, no_progress);
  } catch (...) {
    return -1;
  }
}

} // extern "C"
