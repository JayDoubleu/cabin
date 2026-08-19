/* A small C surface over FreeRDP 3 for Cabin.
 *
 * Everything here takes and returns primitives or opaque pointers, so the Rust
 * side needs no knowledge of FreeRDP struct layouts. That is deliberate:
 * FreeRDP marks most struct members with ALIGN64, bindgen cannot lay those out
 * and silently degrades the structs to opaque blobs. Guessing at the layouts
 * would risk memory corruption for no benefit. See DESIGN.md section 15.
 */
#ifndef CABIN_SHIM_H
#define CABIN_SHIM_H

#include <stdint.h>

typedef struct cabin_rdp cabin_rdp;

/* Result codes. Negative values are failures. */
#define CABIN_OK 0
#define CABIN_ERR_GENERIC (-1)
#define CABIN_ERR_CONNECT (-2)
#define CABIN_ERR_AUTH (-3)
#define CABIN_ERR_DISCONNECTED (-4)
/* The request was valid but the channel that carries it is not attached yet.
 * The caller should hold the request and try again, not discard it. */
#define CABIN_ERR_NOT_READY (-5)

cabin_rdp* cabin_rdp_new(void);
void cabin_rdp_free(cabin_rdp* self);

/* Applies connection settings. Must be called before cabin_rdp_connect. */
int cabin_rdp_configure(cabin_rdp* self, const char* host, uint32_t port, const char* username,
                        const char* password, const char* domain, uint32_t width, uint32_t height);

/* Pins the certificate.
 *
 * Pass the fingerprint recorded from an earlier connection, or NULL on the
 * first connection to accept and record whatever the guest presents. When a
 * fingerprint is set and the guest presents a different one, the connection is
 * refused. */
void cabin_rdp_expect_fingerprint(cabin_rdp* self, const char* fingerprint);

/* The fingerprint the guest presented, or NULL if none has been seen. */
const char* cabin_rdp_seen_fingerprint(cabin_rdp* self);

/* True when the guest presented a fingerprint that did not match. */
int cabin_rdp_fingerprint_changed(cabin_rdp* self);

int cabin_rdp_connect(cabin_rdp* self);

/* Processes pending events. Returns 1 while connected, 0 once disconnected,
 * and a negative code on error. */
int cabin_rdp_poll(cabin_rdp* self, uint32_t timeout_ms);

/* Borrows the current framebuffer. The pointer stays valid until the next
 * resize or free. Returns NULL before the first frame. */
const uint8_t* cabin_rdp_framebuffer(cabin_rdp* self, uint32_t* width, uint32_t* height,
                                     uint32_t* stride);

/* Counts completed frames, so a caller can tell whether anything changed. */
uint64_t cabin_rdp_frame_count(cabin_rdp* self);

/* Asks the server to change the desktop size, over the Display Control
 * channel. The server may grant a different size, so read it back. */
int cabin_rdp_display_ready(cabin_rdp* self);
int cabin_rdp_request_resize(cabin_rdp* self, uint32_t width, uint32_t height);

int cabin_rdp_send_key(cabin_rdp* self, uint32_t scancode, int extended, int down);
int cabin_rdp_send_unicode(cabin_rdp* self, uint16_t code, int down);
int cabin_rdp_send_pointer(cabin_rdp* self, uint32_t x, uint32_t y, uint32_t flags);
int cabin_rdp_send_wheel(cabin_rdp* self, int32_t delta, int horizontal);

void cabin_rdp_disconnect(cabin_rdp* self);

/* The last FreeRDP error code, for reporting. */
uint32_t cabin_rdp_last_error(cabin_rdp* self);

#endif /* CABIN_SHIM_H */
