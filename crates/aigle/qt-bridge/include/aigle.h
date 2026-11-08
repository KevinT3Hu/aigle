#ifndef AIGLE_H
#define AIGLE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* ABI 1. All editor calls are serialized on the owning worker thread.
 * UTF-8 requests/responses use the command protocol documented in qt/README.md.
 * Every returned string must be freed with aigle_string_free, including errors.
 * Null arguments fail safely; arbitrary invalid pointers are not supported.
 * Frames are immutable, independently retained, and may outlive the editor.
 * No Rust or C++ exceptions may cross this boundary. */
typedef struct AigleEditor AigleEditor;
typedef struct AigleFrame AigleFrame;
typedef struct AiglePixels {
  const uint8_t *data;
  size_t length;
  uint32_t width, height, stride;
  uint32_t format; /* 0: BGRA8 sRGB; 1: RGBA16F linear Rec.2020, 1 = 203 nits */
  uint64_t revision;
  double x, y, w, h; /* covered image region */
} AiglePixels;
uint32_t aigle_abi_version(void);
/* Call before creating QApplication; serves parser workers and does not return
 * in a worker. */
void aigle_initialize(void);
AigleEditor *aigle_editor_new(const char *config_json, char **error);
void aigle_editor_free(AigleEditor *editor);
char *aigle_command(AigleEditor *editor, const char *request_json);
void aigle_string_free(char *text);
/* Stateless shared crop math; may run on the GUI thread. Returns owned JSON. */
char *aigle_crop_geometry(const char *request_json);
/* Latest-wins: null means no NEW frame. Original selects the before view. */
AigleFrame *aigle_frame_take(AigleEditor *editor, uint32_t original);
AigleFrame *aigle_thumbnail(AigleEditor *editor, const char *path,
                            uint32_t max_px, char **error);
/* Edited master preview, SDR sRGB, maximum long edge 4096. */
AigleFrame *aigle_comparison_preview(AigleEditor *editor, const char *path,
                                     uint32_t max_px, char **error);
AigleFrame *aigle_comparison_frame_take(AigleEditor *editor, uint32_t index);
AiglePixels aigle_frame_pixels(const AigleFrame *frame);
void aigle_frame_free(AigleFrame *frame);
#ifdef __cplusplus
}
#endif
#endif
