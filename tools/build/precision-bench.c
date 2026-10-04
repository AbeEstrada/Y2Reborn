/* SPDX-License-Identifier: MIT
 * Offline decode/DSP/final-packing CPU measurement of the actual membrane.
 * No ALSA, sink opening or physical-output claim. Compile with the same SDK
 * and FFmpeg libraries as Reborn; run on the Y2 or under QEMU as labelled.
 */
#include "../../crates/reborn-media/native/media.c"

static double cpu_seconds(void) {
  struct timespec t;
  clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &t);
  return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}

int main(int argc, char **argv) {
  if (argc != 5) {
    fputs("Use precision-bench FILE RATE FORMAT(1=S16,2=S32,3=S24) DSP(0|1)\n", stderr);
    return 2;
  }
  int rate = atoi(argv[2]), format = atoi(argv[3]), dsp_on = atoi(argv[4]);
  if ((dsp_on != 0 && dsp_on != 1) || rate < 8000 || rate > 384000 ||
      format < 1 || format > 3)
    return 2;
  RbDspConfig dsp = {.volume = dsp_on ? 73 : 100};
  if (dsp_on) {
    dsp.eq_count = 3;
    dsp.eq[0].frequency_hz = 100; dsp.eq[0].gain_db = 3; dsp.eq[0].q = 1;
    dsp.eq[1].frequency_hz = 1000; dsp.eq[1].gain_db = -2; dsp.eq[1].q = 1;
    dsp.eq[2].frequency_hz = 10000; dsp.eq[2].gain_db = 2; dsp.eq[2].q = 1;
  }
  av_log_set_level(AV_LOG_ERROR);
  double start = cpu_seconds();
  uint64_t total_frames = 0, checksum = 0;
  RbMetadata meta = {0};
  // Repeat complete opens to include bounded decoder/filter setup overhead.
  for (int trial = 0; trial < 5; trial++) {
    RbCancel *cancel = rb_cancel_new();
    RbMedia *media = NULL;
    if (!cancel)
      return 1;
    int rc = rb_media_open(argv[1], rate, format, &dsp, cancel, &media, &meta);
    if (rc < 0) {
      fprintf(stderr, "media open error=%d\n", rc);
      rb_cancel_free(cancel);
      return 1;
    }
    uint8_t output[4096 * 8];
    uint64_t packets = 0, frames = 0;
    int64_t position = 0;
    while ((rc = rb_media_read(media, output, 4096, &packets, &frames, &position)) > 0) {
      total_frames += (uint64_t)rc;
      checksum += output[0] + output[rc * media->output_bytes - 1];
    }
    rb_media_close(media);
    rb_cancel_free(cancel);
    if (rc < 0) {
      fprintf(stderr, "media read error=%d\n", rc);
      return 1;
    }
  }
  double cpu = cpu_seconds() - start;
  double duration = (double)total_frames / rate;
  printf("{\"schema\":\"org.y2linux.precision-bench/v1\","
         "\"physical_output\":false,\"ffmpeg\":\"%s\",\"source_bits\":%u,"
         "\"source_rate_hz\":%u,\"output_rate_hz\":%d,\"format\":%d,"
         "\"dsp_enabled\":%s,\"frames\":%llu,\"checksum\":%llu,"
         "\"cpu_seconds\":%.9f,\"audio_seconds\":%.6f,"
         "\"percent_one_core_realtime\":%.6f}\n",
         rb_media_version(), meta.source_bits, meta.rate, rate, format,
         dsp_on ? "true" : "false", (unsigned long long)total_frames,
         (unsigned long long)checksum, cpu, duration, 100 * cpu / duration);
  return total_frames ? 0 : 1;
}
