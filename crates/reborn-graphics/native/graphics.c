/* SPDX-License-Identifier: MIT
 * GBM/KMS lifetime handling adapted from Y2Linux tools/graphics/gpu-check.c.
 * Rendering and retained handles remain confined to the UI thread. */
#define _POSIX_C_SOURCE 200809L
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <drm_fourcc.h>
#include <errno.h>
#include <fcntl.h>
#include <gbm.h>
#include <poll.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
static double now(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec + t.tv_nsec / 1e9;
}
double rb_graphics_thread_cpu_seconds(void) {
  struct timespec t;
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &t)) return -1;
  return t.tv_sec + t.tv_nsec / 1e9;
}
#include "splash-handoff.h"
struct fb {
  uint32_t id;
  int fd;
};
static void destroy_fb(struct gbm_bo *bo, void *data) {
  (void)bo;
  struct fb *fb = data;
  drmModeRmFB(fb->fd, fb->id);
  free(fb);
}
static struct fb *framebuffer(int fd, struct gbm_bo *bo) {
  struct fb *fb = gbm_bo_get_user_data(bo);
  if (fb)
    return fb;
  if (gbm_bo_get_modifier(bo) != DRM_FORMAT_MOD_LINEAR &&
      gbm_bo_get_modifier(bo) != DRM_FORMAT_MOD_INVALID) {
    return NULL;
  }
  fb = calloc(1, sizeof(*fb));
  if (!fb)
    return NULL;
  fb->fd = fd;
  uint32_t handles[4] = {gbm_bo_get_handle(bo).u32};
  uint32_t pitches[4] = {gbm_bo_get_stride(bo)}, offsets[4] = {0};
  if (drmModeAddFB2(fd, gbm_bo_get_width(bo), gbm_bo_get_height(bo),
                    gbm_bo_get_format(bo), handles, pitches, offsets, &fb->id,
                    0)) {
    free(fb);
    return NULL;
  }
  gbm_bo_set_user_data(bo, fb, destroy_fb);
  return fb;
}
struct flip {
  bool waiting;
  unsigned count, sequence;
  double timestamp;
};
static void flipped(int fd, unsigned seq, unsigned sec, unsigned usec,
                    void *data) {
  (void)fd;
  struct flip *f = data;
  f->waiting = false;
  f->count++;
  f->sequence = seq;
  f->timestamp = sec + usec / 1e6;
}
static int wait_flip(int fd, struct flip *f) {
  double deadline = now() + 3;
  drmEventContext events = {.version = DRM_EVENT_CONTEXT_VERSION,
                            .page_flip_handler = flipped};
  while (f->waiting) {
    struct pollfd pollfd = {.fd = fd, .events = POLLIN};
    int ms = (int)((deadline - now()) * 1000);
    if (ms <= 0) {
      return -1;
    }
    int ret = poll(&pollfd, 1, ms);
    if (ret < 0 && errno == EINTR)
      continue;
    if (ret <= 0 || !(pollfd.revents & POLLIN)) {
      return -1;
    }
    if (drmHandleEvent(fd, &events))
      return -1;
  }
  return 0;
}
static _Thread_local char last_error[256];
const char *rb_graphics_error(void) { return last_error; }
typedef struct {
  int fd, width, height, modeset;
  bool sleeping, failed;
  uint32_t crtc, connector;
  drmModeCrtc *old;
  drmModeModeInfo mode;
  struct gbm_device *gbm;
  struct gbm_surface *surface;
  struct gbm_bo *current;
  struct gbm_bo *pending;
  EGLDisplay display;
  EGLSurface window;
  EGLContext context;
  /* Texture slots, in the order the product UI names them (see rb_graphics_quad):
   * 0 white, 1..3 glyph pages, 4 shape/icon masks, 5 artwork, 6 collection
   * artwork, 7 thumbnail atlas. Every sampled size is kept for UV scaling. */
  GLuint program, vbo, tex[8];
  int tex_w[8], tex_h[8];
  struct flip flip;
  /* Bounded adjacent-texture batching, preserving the original draw order. */
  GLfloat vertices[256 * 6 * 8];
  unsigned vertices_used, draw_calls;
  GLuint batch_texture;
  char info[1024];
} RbGraphics;
/* Normal DRM master operation: stop the CRTC/bridge before idle; retain GBM,
 * EGL and the current buffer so wake restores exactly the owned mode. No
 * platform register access or display-domain power sequencing in userspace. */
int rb_graphics_sleep(RbGraphics *g, int off) {
  if (!g) return -EINVAL;
  if (g->sleeping == !!off) return 0;
  if (g->flip.waiting && wait_flip(g->fd, &g->flip)) return -ETIMEDOUT;
  if (g->modeset || off) {
    int ret;
    if (off) {
      ret = drmModeSetCrtc(g->fd, g->crtc, 0, 0, 0, NULL, 0, NULL);
    } else {
      if (!g->current) return -EINVAL;
      struct fb *f = framebuffer(g->fd, g->current);
      if (!f) return -EIO;
      ret = drmModeSetCrtc(g->fd, g->crtc, f->id, 0, 0,
                           &g->connector, 1, &g->mode);
    }
    if (ret) return -errno;
  }
  g->sleeping = !!off;
  return 0;
}
void rb_graphics_close(RbGraphics *g) {
  if (!g)
    return;
  if (g->modeset && g->failed) {
    /* A missing frame completion must not restart a stale scanout on close. */
    drmModeSetCrtc(g->fd, g->crtc, 0, 0, 0, NULL, 0, NULL);
  } else if (g->modeset && g->old) {
    int r = g->old->mode_valid
                ? drmModeSetCrtc(g->fd, g->old->crtc_id, g->old->buffer_id,
                                 g->old->x, g->old->y, &g->connector, 1,
                                 &g->old->mode)
                : drmModeSetCrtc(g->fd, g->crtc, 0, 0, 0, NULL, 0, NULL);
    if (r)
      drmModeSetCrtc(g->fd, g->crtc, 0, 0, 0, NULL, 0, NULL);
  } else if (g->modeset && !g->sleeping) {
    /* Stop owned scanout before releasing its current GBM buffer. */
    drmModeSetCrtc(g->fd, g->crtc, 0, 0, 0, NULL, 0, NULL);
  }
  if (g->context != EGL_NO_CONTEXT) {
    if (g->program)
      glDeleteProgram(g->program);
    if (g->vbo)
      glDeleteBuffers(1, &g->vbo);
    for (int i = 0; i < 8; i++)
      if (g->tex[i])
        glDeleteTextures(1, &g->tex[i]);
  }
  if (g->current)
    gbm_surface_release_buffer(g->surface, g->current);
  if (g->pending)
    gbm_surface_release_buffer(g->surface, g->pending);
  if (g->display != EGL_NO_DISPLAY) {
    eglMakeCurrent(g->display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    if (g->window != EGL_NO_SURFACE)
      eglDestroySurface(g->display, g->window);
    if (g->context != EGL_NO_CONTEXT)
      eglDestroyContext(g->display, g->context);
    eglTerminate(g->display);
  }
  if (g->surface)
    gbm_surface_destroy(g->surface);
  if (g->gbm)
    gbm_device_destroy(g->gbm);
  drmModeFreeCrtc(g->old);
  if (g->fd >= 0) {
    drmDropMaster(g->fd);
    close(g->fd);
  }
  free(g);
}
static GLuint make_shader(GLenum type, const char *source) {
  GLuint s = glCreateShader(type);
  glShaderSource(s, 1, &source, NULL);
  glCompileShader(s);
  GLint ok;
  glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
  if (!ok) {
    glDeleteShader(s);
    return 0;
  }
  return s;
}
static GLuint texture(int w, int h, const uint8_t *p) {
  GLuint t;
  glGenTextures(1, &t);
  glBindTexture(GL_TEXTURE_2D, t);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE,
               p);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
  return t;
}
/* Thumbnail atlas geometry; keep in step with reborn-graphics/src/lib.rs. */
#define RB_THUMB_W 1024
#define RB_THUMB_H 512
/* Coverage masks (glyph pages, shapes, icons) arrive as one byte per texel and
 * are sampled as luminance-alpha (1,1,1,a): the unchanged fragment program then
 * tints them exactly like the former RGBA atlases at half the video memory. */
static GLuint mask_texture(int w, int h, const uint8_t *a8) {
  uint8_t *la = malloc((size_t)w * h * 2);
  if (!la)
    return 0;
  for (size_t i = 0; i < (size_t)w * h; i++) {
    la[2 * i] = 255;
    la[2 * i + 1] = a8[i];
  }
  GLuint t;
  glGenTextures(1, &t);
  glBindTexture(GL_TEXTURE_2D, t);
  glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_LUMINANCE_ALPHA, w, h, 0,
               GL_LUMINANCE_ALPHA, GL_UNSIGNED_BYTE, la);
  free(la);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
  return t;
}
int rb_graphics_open(RbGraphics **out, const uint8_t *const font_pages[3],
                     const int font_heights[3], const uint8_t *masks,
                     int mask_w, int mask_h) {
  *out = NULL;
  RbGraphics *g = calloc(1, sizeof(*g));
  if (!g)
    return -ENOMEM;
  g->fd = -1;
  int ret = -ENODEV;
  for (int i = 0; i < 16; i++) {
    char path[64];
    snprintf(path, sizeof(path), "/dev/dri/card%d", i);
    int fd = open(path, O_RDWR | O_CLOEXEC);
    if (fd < 0)
      continue;
    drmVersionPtr v = drmGetVersion(fd);
    int found = v && v->name && !strcmp(v->name, "mediatek");
    drmFreeVersion(v);
    if (found) {
      g->fd = fd;
      break;
    }
    close(fd);
  }
  if (g->fd < 0)
    goto fail;
  /* Initialization/rendering can happen while the splash is DRM master.
   * Acquire master only when the first complete frame is ready to scan out. */
  drmModeRes *r = drmModeGetResources(g->fd);
  if (!r)
    goto fail;
  for (int i = 0; i < r->count_connectors && !g->connector; i++) {
    drmModeConnector *c = drmModeGetConnector(g->fd, r->connectors[i]);
    if (c && c->connection == DRM_MODE_CONNECTED && c->count_modes) {
      g->connector = c->connector_id;
      g->mode = c->modes[0];
      for (int k = 0; k < c->count_modes; k++)
        if (c->modes[k].type & DRM_MODE_TYPE_PREFERRED) {
          g->mode = c->modes[k];
          break;
        }
      for (int k = 0; k < c->count_encoders && !g->crtc; k++) {
        drmModeEncoder *e = drmModeGetEncoder(g->fd, c->encoders[k]);
        if (e) {
          if (e->crtc_id)
            g->crtc = e->crtc_id;
          else
            for (int j = 0; j < r->count_crtcs; j++)
              if (e->possible_crtcs & (1U << j)) {
                g->crtc = r->crtcs[j];
                break;
              }
          drmModeFreeEncoder(e);
        }
      }
    }
    drmModeFreeConnector(c);
  }
  drmModeFreeResources(r);
  if (!g->connector || !g->crtc)
    goto fail;
  g->old = drmModeGetCrtc(g->fd, g->crtc);
  g->width = g->mode.hdisplay;
  g->height = g->mode.vdisplay;
  if (g->width > 4096 || g->height > 4096)
    goto fail;
  g->gbm = gbm_create_device(g->fd);
  if (!g->gbm)
    goto fail;
  g->surface = gbm_surface_create(
      g->gbm, g->width, g->height, GBM_FORMAT_XRGB8888,
      GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING | GBM_BO_USE_LINEAR);
  if (!g->surface)
    goto fail;
  PFNEGLGETPLATFORMDISPLAYEXTPROC get =
      (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress(
          "eglGetPlatformDisplayEXT");
  if (!get)
    goto fail;
  g->display = get(EGL_PLATFORM_GBM_KHR, g->gbm, NULL);
  EGLint major, minor;
  if (g->display == EGL_NO_DISPLAY ||
      !eglInitialize(g->display, &major, &minor) ||
      !eglBindAPI(EGL_OPENGL_ES_API))
    goto fail;
  const EGLint attrs[] = {EGL_SURFACE_TYPE,
                          EGL_WINDOW_BIT,
                          EGL_RENDERABLE_TYPE,
                          EGL_OPENGL_ES2_BIT,
                          EGL_RED_SIZE,
                          8,
                          EGL_GREEN_SIZE,
                          8,
                          EGL_BLUE_SIZE,
                          8,
                          EGL_DEPTH_SIZE,
                          0,
                          EGL_STENCIL_SIZE,
                          0,
                          EGL_NONE};
  EGLConfig configs[64], config = NULL;
  EGLint n;
  if (!eglChooseConfig(g->display, attrs, configs, 64, &n))
    goto fail;
  for (int i = 0; i < n; i++) {
    EGLint visual;
    eglGetConfigAttrib(g->display, configs[i], EGL_NATIVE_VISUAL_ID, &visual);
    if (visual == (EGLint)GBM_FORMAT_XRGB8888) {
      config = configs[i];
      break;
    }
  }
  if (!config)
    goto fail;
  const EGLint ca[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
  g->context = eglCreateContext(g->display, config, EGL_NO_CONTEXT, ca);
  g->window = eglCreateWindowSurface(g->display, config,
                                     (EGLNativeWindowType)g->surface, NULL);
  if (g->context == EGL_NO_CONTEXT || g->window == EGL_NO_SURFACE ||
      !eglMakeCurrent(g->display, g->window, g->window, g->context))
    goto fail;
  const char *renderer = (const char *)glGetString(GL_RENDERER);
  if (!renderer || !strstr(renderer, "Mali400")) {
    ret = -ENOTSUP;
    goto fail;
  }
  snprintf(g->info, sizeof(g->info), "%s; EGL %s; %s; %s",
           eglQueryString(g->display, EGL_VENDOR),
           eglQueryString(g->display, EGL_VERSION), renderer,
           glGetString(GL_VERSION));
  eglSwapInterval(g->display, 0);
  const char *vs =
      "attribute vec2 p;attribute vec2 uv;attribute vec4 color;"
      "varying vec2 t;varying vec4 tint;void main(){"
      "gl_Position=vec4(p,0.,1.);t=uv;tint=color;}";
  const char *fs =
      "precision mediump float;varying vec2 t;varying vec4 tint;"
      "uniform sampler2D tex;void main(){gl_FragColor=texture2D(tex,t)*tint;}";
  GLuint v = make_shader(GL_VERTEX_SHADER, vs),
         f = make_shader(GL_FRAGMENT_SHADER, fs);
  if (!v || !f) {
    if (v)
      glDeleteShader(v);
    if (f)
      glDeleteShader(f);
    goto fail;
  }
  g->program = glCreateProgram();
  glAttachShader(g->program, v);
  glAttachShader(g->program, f);
  glBindAttribLocation(g->program, 0, "p");
  glBindAttribLocation(g->program, 1, "uv");
  glBindAttribLocation(g->program, 2, "color");
  glLinkProgram(g->program);
  glDeleteShader(v);
  glDeleteShader(f);
  GLint ok;
  glGetProgramiv(g->program, GL_LINK_STATUS, &ok);
  if (!ok)
    goto fail;
  glUseProgram(g->program);
  glUniform1i(glGetUniformLocation(g->program, "tex"), 0);
  glGenBuffers(1, &g->vbo);
  glBindBuffer(GL_ARRAY_BUFFER, g->vbo);
  glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 8 * sizeof(GLfloat), (void *)0);
  glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 8 * sizeof(GLfloat),
                        (void *)(2 * sizeof(GLfloat)));
  glVertexAttribPointer(2, 4, GL_FLOAT, GL_FALSE, 8 * sizeof(GLfloat),
                        (void *)(4 * sizeof(GLfloat)));
  glEnableVertexAttribArray(0);
  glEnableVertexAttribArray(1);
  glEnableVertexAttribArray(2);
  glEnable(GL_BLEND);
  glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
  glViewport(0, 0, g->width, g->height);
  uint8_t white[] = {255, 255, 255, 255};
  g->tex[0] = texture(1, 1, white);
  g->tex_w[0] = g->tex_h[0] = 1;
  for (int i = 0; i < 3; i++) {
    g->tex[1 + i] = mask_texture(1024, font_heights[i], font_pages[i]);
    g->tex_w[1 + i] = 1024;
    g->tex_h[1 + i] = font_heights[i];
  }
  g->tex[4] = mask_texture(mask_w, mask_h, masks);
  g->tex_w[4] = mask_w;
  g->tex_h[4] = mask_h;
  /* Album art is also used as a full-screen backdrop by the product UI;
   * linear filtering keeps that treatment photographic instead of blocky. */
  g->tex[5] = texture(1, 1, white);
  g->tex[6] = texture(1, 1, white);
  g->tex_w[5] = g->tex_h[5] = g->tex_w[6] = g->tex_h[6] = 1;
  /* Thumbnails: RGBA slots uploaded one at a time by rb_graphics_thumb. */
  g->tex[7] = texture(RB_THUMB_W, RB_THUMB_H, NULL);
  g->tex_w[7] = RB_THUMB_W;
  g->tex_h[7] = RB_THUMB_H;
  for (int i = 5; i < 8; i++) {
    glBindTexture(GL_TEXTURE_2D, g->tex[i]);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
  }
  for (int i = 0; i < 8; i++)
    if (!g->tex[i])
      goto fail;
  if (glGetError() != GL_NO_ERROR)
    goto fail;
  *out = g;
  return 0;
fail:
  snprintf(last_error, sizeof(last_error), "errno=%d (%s), EGL=0x%x", errno,
           strerror(errno), eglGetError());
  rb_graphics_close(g);
  return ret;
}
const char *rb_graphics_info(RbGraphics *g) { return g->info; }
int rb_graphics_width(RbGraphics *g) { return g->width; }
int rb_graphics_height(RbGraphics *g) { return g->height; }
static void flush_batch(RbGraphics *g) {
  if (!g->vertices_used) return;
  glBindTexture(GL_TEXTURE_2D, g->batch_texture);
  glBindBuffer(GL_ARRAY_BUFFER, g->vbo);
  glBufferData(GL_ARRAY_BUFFER, g->vertices_used * 8 * sizeof(GLfloat),
               g->vertices, GL_STREAM_DRAW);
  glDrawArrays(GL_TRIANGLES, 0, g->vertices_used);
  g->draw_calls++;
  g->vertices_used = 0;
}
unsigned rb_graphics_draw_calls(RbGraphics *g) { return g->draw_calls; }
void rb_graphics_submit(RbGraphics *g) { flush_batch(g); }
void rb_graphics_begin(RbGraphics *g) {
  g->vertices_used = 0;
  g->draw_calls = 0;
  glClearColor(.035, .047, .07, 1);
  glClear(GL_COLOR_BUFFER_BIT);
}
/* `tex` selects the texture (0 white, 1..3 glyph pages, 4 masks, 5 artwork,
 * 6 collection artwork, 7 thumbnails); (sx, sy, sw, sh) is the source rectangle
 * in texels of a mask/glyph/thumbnail texture and is ignored otherwise. */
void rb_graphics_quad(RbGraphics *g, float x, float y, float w, float h,
                      uint32_t color, int tex, int sx, int sy, int sw,
                      int sh) {
  if (tex < 0 || tex > 7)
    return;
  GLuint t = g->tex[tex];
  if (g->vertices_used &&
      (g->batch_texture != t || g->vertices_used + 6 > 256 * 6))
    flush_batch(g);
  g->batch_texture = t;
  float u = 0, v = 0, uw = 1, vh = 1;
  if ((tex >= 1 && tex <= 4) || tex == 7) {
    u = (float)sx / g->tex_w[tex];
    v = (float)sy / g->tex_h[tex];
    uw = (float)sw / g->tex_w[tex];
    vh = (float)sh / g->tex_h[tex];
  }
  /* Same box transform and triangle order as the original strip renderer. */
  float cx = (2 * x + w) / g->width - 1;
  float cy = 1 - (2 * y + h) / g->height;
  float hw = w / g->width, hh = h / g->height;
  const float corners[4][4] = {{cx-hw, cy-hh, u, v+vh},
                               {cx+hw, cy-hh, u+uw, v+vh},
                               {cx-hw, cy+hh, u, v},
                               {cx+hw, cy+hh, u+uw, v}};
  const unsigned order[6] = {0, 1, 2, 2, 1, 3};
  const float tint[4] = {((color >> 24) & 255) / 255.f,
                         ((color >> 16) & 255) / 255.f,
                         ((color >> 8) & 255) / 255.f, (color & 255) / 255.f};
  for (unsigned i = 0; i < 6; i++) {
    GLfloat *dst = g->vertices + g->vertices_used++ * 8;
    memcpy(dst, corners[order[i]], 4 * sizeof(GLfloat));
    memcpy(dst + 4, tint, 4 * sizeof(GLfloat));
  }
}
/* Upload one 98x98 RGBA thumbnail cell (a 96x96 image with a replicated
 * one-pixel gutter) at atlas cell (cx, cy). */
int rb_graphics_thumb(RbGraphics *g, int cx, int cy, const uint8_t *rgba) {
  if (cx < 0 || cy < 0 || (cx + 1) * 98 > RB_THUMB_W ||
      (cy + 1) * 98 > RB_THUMB_H)
    return -EINVAL;
  glBindTexture(GL_TEXTURE_2D, g->tex[7]);
  glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
  glTexSubImage2D(GL_TEXTURE_2D, 0, cx * 98, cy * 98, 98, 98, GL_RGBA,
                  GL_UNSIGNED_BYTE, rgba);
  return glGetError() == GL_NO_ERROR ? 0 : -EIO;
}
int rb_graphics_art(RbGraphics *g, const uint8_t *p, int collection) {
  glBindTexture(GL_TEXTURE_2D, collection ? g->tex[6] : g->tex[5]);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 160, 160, 0, GL_RGBA,
               GL_UNSIGNED_BYTE, p);
  return glGetError() == GL_NO_ERROR ? 0 : -EIO;
}
int rb_graphics_test(RbGraphics *g) {
  GLuint t = texture(8, 8, NULL), fb;
  glGenFramebuffers(1, &fb);
  glBindFramebuffer(GL_FRAMEBUFFER, fb);
  glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, t,
                         0);
  int ok = glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
  uint8_t p[4] = {0};
  if (ok) {
    glClearColor(1, 0, 0, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    glReadPixels(0, 0, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, p);
    ok = p[0] > 240 && p[1] < 8 && p[2] < 8 && glGetError() == GL_NO_ERROR;
  }
  glBindFramebuffer(GL_FRAMEBUFFER, 0);
  glDeleteFramebuffers(1, &fb);
  glDeleteTextures(1, &t);
  glViewport(0, 0, g->width, g->height);
  return ok ? 0 : -EIO;
}
int rb_graphics_readback(RbGraphics *g, uint8_t *pixels) {
  flush_batch(g);
  glReadPixels(0, 0, g->width, g->height, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
  return glGetError() == GL_NO_ERROR ? 0 : -EIO;
}
int rb_graphics_present(RbGraphics *g) {
  flush_batch(g);
  if (g->failed) return -EIO;
  if (g->sleeping) return -EHOSTDOWN;
  if (glGetError() != GL_NO_ERROR || !eglSwapBuffers(g->display, g->window))
    return -EIO;
  struct gbm_bo *bo = gbm_surface_lock_front_buffer(g->surface);
  if (!bo)
    return -ENOMEM;
  struct fb *f = framebuffer(g->fd, bo);
  if (!f) {
    gbm_surface_release_buffer(g->surface, bo);
    return -EIO;
  }
  int handoff = -1, error;
  if (!g->modeset) {
    handoff = rb_splash_begin("/run/reborn-splash/control.sock");
    if (handoff < -1) {
      gbm_surface_release_buffer(g->surface, bo);
      return handoff + 1;
    }
    if (drmSetMaster(g->fd) || drmModeSetCrtc(g->fd, g->crtc, f->id, 0, 0, &g->connector, 1,
                       &g->mode)) {
      int error = errno;
      if (handoff >= 0) close(handoff);
      gbm_surface_release_buffer(g->surface, bo);
      return -error;
    }
    g->modeset = 1;
  }
  /* SetCrtc can return success after a kernel vblank timeout. Require an
   * actual page-flip completion even for the first frame, before readiness
   * or releasing the splash. Retain both buffers until stopped on failure. */
  g->pending = bo;
  g->flip.waiting = true;
  if (drmModePageFlip(g->fd, g->crtc, f->id, DRM_MODE_PAGE_FLIP_EVENT,
                      &g->flip)) {
    error = -errno;
    g->flip.waiting = false;
    goto failed;
  }
  if (wait_flip(g->fd, &g->flip)) {
    error = -ETIMEDOUT;
    goto failed;
  }
  if (handoff >= 0) {
    /* The old splash FB is destroyed after our acknowledgement. */
    drmModeFreeCrtc(g->old); g->old = NULL;
    rb_splash_presented(handoff);
  }
  if (g->current)
    gbm_surface_release_buffer(g->surface, g->current);
  g->current = bo;
  g->pending = NULL;
  return 0;
failed:
  if (handoff >= 0) close(handoff);
  g->failed = true;
  return error;
}
