/* Render a quad list with the production GL code on any host GLES2 context
 * (Mesa, EGL surfaceless pbuffer) and write the frame as a binary PPM. Used by
 * tools/preview/gles_compare.py to check textures, tinting and UV mapping
 * against the Python rasteriser. Not part of the player. */
#define _POSIX_C_SOURCE 200809L
#define eglSwapBuffers fake_swap
#include "../native/graphics.c"
#undef eglSwapBuffers
EGLBoolean fake_swap(EGLDisplay d, EGLSurface s) { (void)d; (void)s; return EGL_TRUE; }

static uint8_t *slurp(const char *path, size_t *size) {
  FILE *f = fopen(path, "rb");
  if (!f) { perror(path); exit(2); }
  fseek(f, 0, SEEK_END);
  *size = (size_t)ftell(f);
  fseek(f, 0, SEEK_SET);
  uint8_t *data = malloc(*size);
  if (fread(data, 1, *size, f) != *size) exit(2);
  fclose(f);
  return data;
}

int main(int argc, char **argv) {
  if (argc != 4) return 2;
  char path[512];
  size_t sizes[4];
  const char *names[3] = {"fonts/reborn-regular.a8", "fonts/reborn-semibold.a8", "fonts/reborn-fallback.a8"};
  const uint8_t *pages[3];
  int heights[3];
  for (int i = 0; i < 3; i++) {
    snprintf(path, sizeof path, "%s/%s", argv[1], names[i]);
    pages[i] = slurp(path, &sizes[i]);
    heights[i] = (int)(sizes[i] / 1024);
  }
  snprintf(path, sizeof path, "%s/icons/reborn-masks.a8", argv[1]);
  const uint8_t *masks = slurp(path, &sizes[3]);
  int mask_w = 512, mask_h = (int)(sizes[3] / 512);

  EGLDisplay display = eglGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
  EGLint major, minor;
  if (!eglInitialize(display, &major, &minor)) return 3;
  eglBindAPI(EGL_OPENGL_ES_API);
  const EGLint cfg[] = {EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
                        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE};
  EGLConfig config;
  EGLint n;
  if (!eglChooseConfig(display, cfg, &config, 1, &n) || n < 1) return 4;
  const EGLint pb[] = {EGL_WIDTH, 480, EGL_HEIGHT, 360, EGL_NONE};
  EGLSurface surface = eglCreatePbufferSurface(display, config, pb);
  const EGLint ca[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
  EGLContext context = eglCreateContext(display, config, EGL_NO_CONTEXT, ca);
  if (!eglMakeCurrent(display, surface, surface, context)) return 5;

  RbGraphics *g = calloc(1, sizeof *g);
  g->width = 480;
  g->height = 360;
  glViewport(0, 0, 480, 360);
  glEnable(GL_BLEND);
  glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
  if (gl_setup(g, pages, heights, masks, mask_w, mask_h) < 0) { fputs("gl_setup failed\n", stderr); return 6; }
  rb_graphics_begin(g);

  FILE *quads = fopen(argv[2], "r");
  char line[1024];
  while (fgets(line, sizeof line, quads)) {
    if (line[0] == 'Q') {
      float x, y, w, h;
      unsigned color;
      int tex, sx, sy, sw, sh;
      if (sscanf(line + 1, "%f %f %f %f %u %d %d %d %d %d", &x, &y, &w, &h, &color, &tex, &sx, &sy, &sw, &sh) == 10)
        rb_graphics_quad(g, x, y, w, h, color, tex, sx, sy, sw, sh);
    } else if (line[0] == 'W') {
      float c[8];
      unsigned k[4];
      int tex, sx, sy, sw, sh;
      if (sscanf(line + 1, "%f %f %f %f %f %f %f %f %u %u %u %u %d %d %d %d %d", &c[0], &c[1], &c[2], &c[3], &c[4],
                 &c[5], &c[6], &c[7], &k[0], &k[1], &k[2], &k[3], &tex, &sx, &sy, &sw, &sh) == 17) {
        const uint32_t colors[4] = {k[0], k[1], k[2], k[3]};
        rb_graphics_quad_warp(g, c, colors, tex, sx, sy, sw, sh);
      }
    } else if (line[0] == 'C') {
      char file[400];
      if (sscanf(line + 1, "%399s", file) == 1) {
        size_t size;
        uint8_t *art = slurp(file, &size);
        if (rb_graphics_art(g, art, 1) < 0) return 8;
        free(art);
      }
    } else if (line[0] == 'T') {
      int cx, cy;
      char file[400];
      if (sscanf(line + 1, "%d %d %399s", &cx, &cy, file) == 3) {
        size_t size;
        uint8_t *cell = slurp(file, &size);
        if (rb_graphics_thumb(g, cx, cy, cell) < 0) return 7;
        free(cell);
      }
    } else if (line[0] == 'G') {
      /* The runtime glyph page: one 1024x1024 A8 file, uploaded whole. */
      char file[400];
      if (sscanf(line + 1, "%399s", file) == 1) {
        size_t size;
        uint8_t *page = slurp(file, &size);
        if (size != (size_t)RB_GLYPHS * RB_GLYPHS || rb_graphics_glyphs(g, 0, RB_GLYPHS, page) < 0) return 10;
        free(page);
      }
    } else if (line[0] == 'A') {
      char file[400];
      if (sscanf(line + 1, "%399s", file) == 1) {
        size_t size;
        uint8_t *art = slurp(file, &size);
        if (rb_graphics_art(g, art, 0) < 0) return 8;
        free(art);
      }
    }
  }
  fclose(quads);
  uint8_t *pixels = malloc(480 * 360 * 4);
  if (rb_graphics_readback(g, pixels) < 0) return 9;
  FILE *out = fopen(argv[3], "wb");
  fprintf(out, "P6\n480 360\n255\n");
  /* GL's origin is bottom-left. */
  for (int y = 359; y >= 0; y--)
    for (int x = 0; x < 480; x++) fwrite(pixels + ((size_t)y * 480 + x) * 4, 1, 3, out);
  fclose(out);
  return 0;
}
