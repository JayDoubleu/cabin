/* See shim.h for why this layer exists. */
#include "shim.h"

#include <freerdp/freerdp.h>
#include <freerdp/client.h>
#include <freerdp/settings.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/input.h>
#include <freerdp/channels/channels.h>
#include <freerdp/client/channels.h>
#include <freerdp/client/disp.h>
#include <freerdp/client/rdpgfx.h>
#include <freerdp/gdi/gfx.h>
#include <freerdp/event.h>
#include <freerdp/error.h>

#include <stdlib.h>
#include <string.h>

/* The context FreeRDP allocates for us. The first member must be rdpContext,
 * because FreeRDP casts between the two. */
typedef struct
{
	rdpContext context;
	struct cabin_rdp* owner;
} cabinContext;

struct cabin_rdp
{
	cabinContext* ctx;
	uint64_t frames;
	uint32_t last_error;
	int connected;
	DispClientContext* disp;
	pEndPaint original_end_paint;
};

static struct cabin_rdp* owner_of(rdpContext* context)
{
	return ((cabinContext*)context)->owner;
}

/* ---------- FreeRDP callbacks ---------- */

static BOOL cabin_end_paint(rdpContext* context);

/* Accept the guest's self-signed certificate.
 *
 * Cabin generates one certificate per container and the user never sees a
 * certificate authority, so a chain check would always fail. Pinning the
 * fingerprint is the right long-term answer and is tracked in DESIGN.md
 * section 8. Until then the transport is encrypted but unauthenticated. */
static DWORD cabin_verify_certificate(freerdp* instance, const char* host, UINT16 port,
                                      const char* common_name, const char* subject,
                                      const char* issuer, const char* fingerprint, DWORD flags)
{
	(void)instance;
	(void)host;
	(void)port;
	(void)common_name;
	(void)subject;
	(void)issuer;
	(void)fingerprint;
	(void)flags;
	return 2; /* accept for this session only */
}

static DWORD cabin_verify_changed_certificate(freerdp* instance, const char* host, UINT16 port,
                                              const char* common_name, const char* subject,
                                              const char* issuer, const char* fingerprint,
                                              const char* old_subject, const char* old_issuer,
                                              const char* old_fingerprint, DWORD flags)
{
	(void)old_subject;
	(void)old_issuer;
	(void)old_fingerprint;
	return cabin_verify_certificate(instance, host, port, common_name, subject, issuer, fingerprint,
	                                flags);
}

static BOOL cabin_pre_connect(freerdp* instance)
{
	rdpSettings* settings = instance->context->settings;

	/* gnome-remote-desktop refuses any client that does not offer the
	 * graphics pipeline, and the pipeline rides on the dynamic channel
	 * transport. Both are mandatory rather than nice to have. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, TRUE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_SupportDynamicChannels, TRUE))
		return FALSE;

	/* The guest also requires 32-bit colour, fastpath output and a pointer
	 * cache, and rejects the connection without them. */
	if (!freerdp_settings_set_uint32(settings, FreeRDP_ColorDepth, 32))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_FastPathOutput, TRUE))
		return FALSE;
	if (!freerdp_settings_set_uint32(settings, FreeRDP_PointerCacheSize, 32))
		return FALSE;

	/* Display Control, so the desktop can follow the window size. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_SupportDisplayControl, TRUE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_DynamicResolutionUpdate, TRUE))
		return FALSE;

	/* Clipboard redirection must be switched OFF explicitly.
	 *
	 * gnome-remote-desktop mounts a FUSE filesystem the moment a client
	 * advertises cliprdr. That mount needs CAP_SYS_ADMIN, which a rootless
	 * container cannot have, and the daemon calls abort() when it fails. So a
	 * client that advertises the clipboard kills the guest session outright.
	 *
	 * Note that CLIPRDR is a STATIC virtual channel, so it never appears in the
	 * "Loading Dynamic Virtual Channel" log lines. Seeing only ainput, rdpgfx,
	 * disp and rdpsnd there does not mean the clipboard is off. FreeRDP enables
	 * it by default and it must be turned off by name.
	 *
	 * See DESIGN.md section 15.2. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_RedirectClipboard, FALSE))
		return FALSE;

	/* Pin the graphics pipeline to the codecs that are actually stable here.
	 *
	 * Fedora builds FreeRDP with WITH_GFX_AV1=ON, which the library itself
	 * announces at runtime as experimental and warns "might crash the
	 * application". It does: with AV1 left enabled the client segfaults inside
	 * FreeRDP's event handling as soon as the guest starts sending frames.
	 * RemoteFX Progressive is what gnome-remote-desktop falls back to, and it
	 * is well exercised. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_GfxCodecAV1, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_GfxProgressive, TRUE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_GfxAVC444, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_GfxAVC444v2, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_GfxH264, FALSE))
		return FALSE;

	/* Turn off everything Cabin does not use. Each of these is a channel with
	 * its own plugin, its own state and its own failure modes, and none of them
	 * is needed to show a desktop. Audio is planned but not wired up yet, so it
	 * stays off rather than half on. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_AudioPlayback, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_AudioCapture, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_DeviceRedirection, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_RedirectDrives, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_RedirectPrinters, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_RedirectSmartCards, FALSE))
		return FALSE;
	if (!freerdp_settings_set_bool(settings, FreeRDP_SupportEchoChannel, FALSE))
		return FALSE;

	if (!freerdp_client_load_addins(instance->context->channels, settings))
		return FALSE;

	return TRUE;
}

static BOOL cabin_post_connect(freerdp* instance)
{
	struct cabin_rdp* self = owner_of(instance->context);

	/* BGRA32 matches what GDK wants for a memory texture on little endian,
	 * so no conversion is needed on the way to the widget. */
	if (!gdi_init(instance, PIXEL_FORMAT_BGRA32))
		return FALSE;

	/* Chain rather than replace. gdi_init installs its own EndPaint and the
	 * display breaks if that one is dropped. */
	self->original_end_paint = instance->context->update->EndPaint;
	instance->context->update->EndPaint = cabin_end_paint;

	self->connected = 1;
	return TRUE;
}

static void cabin_post_disconnect(freerdp* instance)
{
	struct cabin_rdp* self = owner_of(instance->context);
	self->connected = 0;
	gdi_free(instance);
}

/* Counts a frame each time the server finishes painting. */
static BOOL cabin_end_paint(rdpContext* context)
{
	struct cabin_rdp* self = owner_of(context);
	self->frames++;
	if (self->original_end_paint)
		return self->original_end_paint(context);
	return TRUE;
}

/* The graphics pipeline delivers its surfaces to a channel, not to the legacy
 * drawing orders. Unless the channel is handed to the GDI layer here, FreeRDP
 * decodes frames that never reach gdi->primary_buffer, and the framebuffer
 * stays blank while the session otherwise looks healthy. */
static void cabin_channel_connected(void* context, const ChannelConnectedEventArgs* e)
{
	rdpContext* ctx = (rdpContext*)context;
	struct cabin_rdp* self = owner_of(ctx);

	if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0)
	{
		gdi_graphics_pipeline_init(ctx->gdi, (RdpgfxClientContext*)e->pInterface);
	}
	else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0)
	{
		self->disp = (DispClientContext*)e->pInterface;
	}
}

static void cabin_channel_disconnected(void* context, const ChannelDisconnectedEventArgs* e)
{
	rdpContext* ctx = (rdpContext*)context;
	struct cabin_rdp* self = owner_of(ctx);

	if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0)
	{
		gdi_graphics_pipeline_uninit(ctx->gdi, (RdpgfxClientContext*)e->pInterface);
	}
	else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0)
	{
		self->disp = NULL;
	}
}

/* ---------- client entry points ---------- */

static BOOL cabin_client_new(freerdp* instance, rdpContext* context)
{
	(void)context;
	instance->PreConnect = cabin_pre_connect;
	instance->PostConnect = cabin_post_connect;
	instance->PostDisconnect = cabin_post_disconnect;
	instance->VerifyCertificateEx = cabin_verify_certificate;
	instance->VerifyChangedCertificateEx = cabin_verify_changed_certificate;

	if (PubSub_SubscribeChannelConnected(context->pubSub, cabin_channel_connected) != CHANNEL_RC_OK)
		return FALSE;
	if (PubSub_SubscribeChannelDisconnected(context->pubSub, cabin_channel_disconnected) !=
	    CHANNEL_RC_OK)
		return FALSE;
	return TRUE;
}

static void cabin_client_free(freerdp* instance, rdpContext* context)
{
	(void)instance;
	(void)context;
}

/* ---------- public surface ---------- */

cabin_rdp* cabin_rdp_new(void)
{
	struct cabin_rdp* self = calloc(1, sizeof(struct cabin_rdp));
	if (!self)
		return NULL;

	RDP_CLIENT_ENTRY_POINTS entry;
	memset(&entry, 0, sizeof(entry));
	entry.Version = RDP_CLIENT_INTERFACE_VERSION;
	entry.Size = sizeof(entry);
	entry.ContextSize = sizeof(cabinContext);
	entry.ClientNew = cabin_client_new;
	entry.ClientFree = cabin_client_free;

	rdpContext* context = freerdp_client_context_new(&entry);
	if (!context)
	{
		free(self);
		return NULL;
	}

	self->ctx = (cabinContext*)context;
	self->ctx->owner = self;
	return self;
}

void cabin_rdp_free(cabin_rdp* self)
{
	if (!self)
		return;
	if (self->ctx)
		freerdp_client_context_free(&self->ctx->context);
	free(self);
}

int cabin_rdp_configure(cabin_rdp* self, const char* host, uint32_t port, const char* username,
                        const char* password, const char* domain, uint32_t width, uint32_t height)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;

	rdpSettings* settings = self->ctx->context.settings;

	if (!freerdp_settings_set_string(settings, FreeRDP_ServerHostname, host))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_uint32(settings, FreeRDP_ServerPort, port))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_string(settings, FreeRDP_Username, username))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_string(settings, FreeRDP_Password, password))
		return CABIN_ERR_GENERIC;
	if (domain && domain[0] &&
	    !freerdp_settings_set_string(settings, FreeRDP_Domain, domain))
		return CABIN_ERR_GENERIC;

	if (!freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth, width))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_uint32(settings, FreeRDP_DesktopHeight, height))
		return CABIN_ERR_GENERIC;

	/* The guest refuses plain TLS with HYBRID_REQUIRED_BY_SERVER, so NLA is
	 * the only path that works. See DESIGN.md section 12.4. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_NlaSecurity, TRUE))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_bool(settings, FreeRDP_TlsSecurity, TRUE))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_bool(settings, FreeRDP_RdpSecurity, FALSE))
		return CABIN_ERR_GENERIC;

	/* Cabin generates a fresh certificate per container. See the note on
	 * cabin_verify_certificate. */
	if (!freerdp_settings_set_bool(settings, FreeRDP_IgnoreCertificate, TRUE))
		return CABIN_ERR_GENERIC;
	if (!freerdp_settings_set_bool(settings, FreeRDP_AutoAcceptCertificate, TRUE))
		return CABIN_ERR_GENERIC;

	return CABIN_OK;
}

int cabin_rdp_connect(cabin_rdp* self)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;

	freerdp* instance = self->ctx->context.instance;
	if (!freerdp_connect(instance))
	{
		self->last_error = freerdp_get_last_error(&self->ctx->context);
		switch (self->last_error)
		{
			case FREERDP_ERROR_CONNECT_LOGON_FAILURE:
			case FREERDP_ERROR_CONNECT_ACCOUNT_DISABLED:
			case FREERDP_ERROR_CONNECT_PASSWORD_EXPIRED:
				return CABIN_ERR_AUTH;
			default:
				return CABIN_ERR_CONNECT;
		}
	}

	return CABIN_OK;
}

int cabin_rdp_poll(cabin_rdp* self, uint32_t timeout_ms)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;

	freerdp* instance = self->ctx->context.instance;
	if (freerdp_shall_disconnect_context(&self->ctx->context))
		return 0;

	HANDLE handles[64];
	DWORD count = freerdp_get_event_handles(&self->ctx->context, handles,
	                                        sizeof(handles) / sizeof(handles[0]));
	if (count == 0)
		return CABIN_ERR_GENERIC;

	DWORD status = WaitForMultipleObjects(count, handles, FALSE, timeout_ms);
	if (status == WAIT_FAILED)
		return CABIN_ERR_GENERIC;

	if (!freerdp_check_event_handles(&self->ctx->context))
	{
		self->last_error = freerdp_get_last_error(&self->ctx->context);
		return freerdp_shall_disconnect_context(&self->ctx->context) ? 0 : CABIN_ERR_DISCONNECTED;
	}

	(void)instance;
	return 1;
}

const uint8_t* cabin_rdp_framebuffer(cabin_rdp* self, uint32_t* width, uint32_t* height,
                                     uint32_t* stride)
{
	if (!self || !self->ctx)
		return NULL;

	rdpGdi* gdi = self->ctx->context.gdi;
	if (!gdi || !gdi->primary_buffer)
		return NULL;

	if (width)
		*width = (uint32_t)gdi->width;
	if (height)
		*height = (uint32_t)gdi->height;
	if (stride)
		*stride = (uint32_t)gdi->stride;
	return (const uint8_t*)gdi->primary_buffer;
}

uint64_t cabin_rdp_frame_count(cabin_rdp* self)
{
	return self ? self->frames : 0;
}

int cabin_rdp_request_resize(cabin_rdp* self, uint32_t width, uint32_t height)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;
	if (!self->disp)
		return CABIN_ERR_GENERIC;

	DISPLAY_CONTROL_MONITOR_LAYOUT layout;
	memset(&layout, 0, sizeof(layout));
	layout.Flags = DISPLAY_CONTROL_MONITOR_PRIMARY;
	layout.Width = width;
	layout.Height = height;
	layout.Orientation = ORIENTATION_LANDSCAPE;
	layout.DesktopScaleFactor = 100;
	layout.DeviceScaleFactor = 100;

	return self->disp->SendMonitorLayout(self->disp, 1, &layout) == CHANNEL_RC_OK
	           ? CABIN_OK
	           : CABIN_ERR_GENERIC;
}

int cabin_rdp_send_key(cabin_rdp* self, uint32_t scancode, int extended, int down)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;
	UINT16 flags = down ? KBD_FLAGS_DOWN : KBD_FLAGS_RELEASE;
	if (extended)
		flags |= KBD_FLAGS_EXTENDED;
	return freerdp_input_send_keyboard_event(self->ctx->context.input, flags, (UINT8)scancode)
	           ? CABIN_OK
	           : CABIN_ERR_GENERIC;
}

int cabin_rdp_send_unicode(cabin_rdp* self, uint16_t code, int down)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;
	UINT16 flags = down ? KBD_FLAGS_DOWN : KBD_FLAGS_RELEASE;
	return freerdp_input_send_unicode_keyboard_event(self->ctx->context.input, flags, code)
	           ? CABIN_OK
	           : CABIN_ERR_GENERIC;
}

int cabin_rdp_send_pointer(cabin_rdp* self, uint32_t x, uint32_t y, uint32_t flags)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;
	return freerdp_input_send_mouse_event(self->ctx->context.input, (UINT16)flags, (UINT16)x,
	                                      (UINT16)y)
	           ? CABIN_OK
	           : CABIN_ERR_GENERIC;
}

int cabin_rdp_send_wheel(cabin_rdp* self, int32_t delta, int horizontal)
{
	if (!self || !self->ctx)
		return CABIN_ERR_GENERIC;
	UINT16 flags = horizontal ? PTR_FLAGS_HWHEEL : PTR_FLAGS_WHEEL;
	if (delta < 0)
	{
		flags |= PTR_FLAGS_WHEEL_NEGATIVE;
		delta = -delta;
	}
	flags |= (UINT16)(delta & 0xFF);
	return freerdp_input_send_mouse_event(self->ctx->context.input, flags, 0, 0) ? CABIN_OK
	                                                                            : CABIN_ERR_GENERIC;
}

void cabin_rdp_disconnect(cabin_rdp* self)
{
	if (!self || !self->ctx)
		return;
	freerdp_disconnect(self->ctx->context.instance);
	self->connected = 0;
}

uint32_t cabin_rdp_last_error(cabin_rdp* self)
{
	return self ? self->last_error : 0;
}
