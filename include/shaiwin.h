/* shaiwin.h — C ABI over the platform-agnostic ShaiWin core.
 *
 * Build:  cargo build -p shaiwin-ffi --release
 * Output: target/release/shaiwin.dll / libshaiwin.a / libshaiwin.so
 *
 * Every pointer returned here is owned by the caller and must be freed with the
 * matching `_free` function. Nothing allocates on the Swift side.
 */
#ifndef SHAIWIN_H
#define SHAIWIN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

const char *shaiwin_version(void);

/* One line, on one monitor. `width` is in device pixels, `scale` is the
 * backing scale factor (2.0 on a Retina display). */
void *shaiwin_layout_new(float width, float scale);
void  shaiwin_layout_free(void *layout);

float shaiwin_layout_slot_x(const void *layout, size_t index, size_t count);
float shaiwin_layout_rope_y(const void *layout, float x);
float shaiwin_layout_panel_height(const void *layout);

/* A card's flight from a window rectangle to its slot, or back. */
void *shaiwin_flight_new(float from_x, float from_y, float to_x, float to_y,
                         float lift, float duration);
int   shaiwin_flight_step(void *flight, float dt);      /* 1 = still flying */
void  shaiwin_flight_position(const void *flight, float out[2]);
void  shaiwin_flight_free(void *flight);

#ifdef __cplusplus
}
#endif

#endif /* SHAIWIN_H */
