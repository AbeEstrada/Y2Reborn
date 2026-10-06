/* SPDX-License-Identifier: MIT
 * Dump the actual decode/float DSP/final integer membrane for comparison with
 * the live ALSA ring. This helper never opens a sink or plays audio.
 */
#include "../../crates/reborn-media/native/media.c"

int main(int argc, char **argv) {
  if (argc != 5) {
    fputs("Use precision-probe SOURCE RATE FORMAT(1=S16,2=S32,3=S24) RAW_OUTPUT\n", stderr);
    return 2;
  }
  int rate = atoi(argv[2]), format = atoi(argv[3]);
  if ((rate != 44100 && rate != 48000) || format < 1 || format > 3)
    return 2;
  RbCancel *cancel = rb_cancel_new();
  RbMedia *media = NULL;
  RbMetadata metadata = {0};
  RbDspConfig dsp = {.volume = 100};
  av_log_set_level(AV_LOG_ERROR);
  if (!cancel)
    return 1;
  int rc = rb_media_open(argv[1], rate, format, &dsp, cancel, &media, &metadata);
  if (rc < 0) {
    fprintf(stderr, "open=%d\n", rc);
    rb_cancel_free(cancel);
    return 1;
  }
  FILE *out = fopen(argv[4], "wb");
  uint64_t total = 0, packets = 0, frames = 0;
  int64_t position = 0;
  uint8_t bytes[4096 * 8];
  if (!out) {
    rb_media_close(media);
    rb_cancel_free(cancel);
    return 1;
  }
  while ((rc = rb_media_read(media, bytes, 4096, &packets, &frames, &position)) > 0) {
    size_t size = (size_t)rc * media->output_bytes;
    if (fwrite(bytes, 1, size, out) != size) {
      rc = -1;
      break;
    }
    total += (uint64_t)rc;
  }
  int close_error = fclose(out);
  printf("{\"source_bits\":%u,\"source_rate_hz\":%u,\"format\":%d,"
         "\"frames\":%llu,\"physical_output\":false,\"ffmpeg\":\"%s\"}\n",
         metadata.source_bits, metadata.rate, format,
         (unsigned long long)total, rb_media_version());
  rb_media_close(media);
  rb_cancel_free(cancel);
  return rc < 0 || close_error || !total;
}
