/* SPDX-License-Identifier: MIT
 * Host ownership stress using the production membrane and direct FFmpeg APIs.
 * Build with -fsanitize=address,undefined and the six pkg-config FFmpeg libraries.
 * Usage: media_sanitize fixture... (no CLI decoder subprocess).
 */
#include "../../crates/reborn-media/native/media.c"
int main(int argc, char **argv) {
  rb_media_limits();
  rb_media_logging(NULL, NULL);
  for (int pass = 0; pass < 20; pass++) {
    for (int i = 1; i < argc; i++) {
      RbCancel *cancel = rb_cancel_new();
      RbMedia *media = NULL;
      RbMetadata metadata;
      uint8_t pixels[160 * 160 * 4];
      if (!cancel) return 2;
      if (rb_media_inspect(argv[i], cancel, &media, &metadata) >= 0) {
        if (metadata.artwork) (void)rb_media_art(media, pixels, 160);
        rb_media_close(media);
      }
      RbDspConfig dsp = {.volume = 100};
      media = NULL;
      if (rb_media_open(argv[i], 48000, 2, &dsp, cancel, &media, &metadata) >= 0) {
        uint8_t output[1024 * 8];
        uint64_t packets, frames;
        int64_t position;
        for (int j = 0; j < 3; j++)
          if (rb_media_read(media, output, 1024, &packets, &frames, &position) <= 0) break;
        rb_cancel_set(cancel);
        (void)rb_media_read(media, output, 1024, &packets, &frames, &position);
        rb_media_close(media);
      }
      (void)rb_media_art_file(argv[i], pixels, 160);
      rb_cancel_free(cancel);
    }
  }
  printf("ownership stress passed: %d fixtures, 20 iterations\n", argc - 1);
  return 0;
}
