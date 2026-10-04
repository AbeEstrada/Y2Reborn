/* Product write-size probe: native 512-frame wired PCM, silence, no gain work.
 * Compile with the production SDK and -lasound. Caller must own the device
 * lease and stop/restore Reborn. It uses the normal sink/mixer restoration. */
#include "../../crates/reborn-audio/native/audio.c"
#include <time.h>

static double seconds(clockid_t clock) {
  struct timespec value;
  clock_gettime(clock, &value);
  return (double)value.tv_sec + (double)value.tv_nsec / 1e9;
}

int main(int argc, char **argv) {
  int partial_wait = argc == 2 && !strcmp(argv[1], "partial-wait");
  if (argc != 2 || (!partial_wait && strcmp(argv[1], "unpaced")))
    return 2;
  RbSink *sink = NULL;
  RbParams params = {0};
  char device[128];
  int result = rb_wired_device(device, sizeof(device));
  if (result < 0)
    return 3;
  result = rb_sink_open(device, 48000, 1, 1, &sink, &params);
  if (result < 0) {
    fprintf(stderr, "%s\n", rb_alsa_error(result));
    return 4;
  }
  uint8_t silence[4096] = {0};
  unsigned long calls = 0, frames = 0, full = 0, partial = 0, zero = 0;
  unsigned long again = 0, errors = 0, waits = 0, small = 0;
  int minimum = 512, maximum = 0;
  double started = seconds(CLOCK_MONOTONIC);
  double cpu = seconds(CLOCK_THREAD_CPUTIME_ID);
  while (seconds(CLOCK_MONOTONIC) - started < 1.0) {
    int n = rb_sink_write(sink, silence, 512);
    calls++;
    if (n > 0) {
      frames += (unsigned)n;
      if (n < minimum) minimum = n;
      if (n > maximum) maximum = n;
      if (n == 512) full++; else partial++;
      if (n < 64) small++;
    } else if (n == 0) zero++;
    else if (n == -EAGAIN) again++;
    else { errors++; break; }
    if (n == -EAGAIN || (partial_wait && n >= 0 && n < 512)) {
      rb_sink_wait(sink, 20);
      waits++;
    }
  }
  cpu = seconds(CLOCK_THREAD_CPUTIME_ID) - cpu;
  double elapsed = seconds(CLOCK_MONOTONIC) - started;
  rb_sink_close(sink);
  printf("{\"mode\":\"%s\",\"rate\":%u,\"period\":%u,\"buffer\":%u,"
         "\"requested_frames\":512,\"seconds\":%.6f,\"thread_cpu_ms\":%.6f,"
         "\"calls\":%lu,\"accepted_frames\":%lu,\"full\":%lu,"
         "\"partial\":%lu,\"zero\":%lu,\"again\":%lu,\"errors\":%lu,"
         "\"waits\":%lu,\"writes_below_64_frames\":%lu,"
         "\"minimum_positive_frames\":%d,\"maximum_positive_frames\":%d}\n",
         argv[1], params.rate, params.period, params.buffer, elapsed, cpu * 1000,
         calls, frames, full, partial, zero, again, errors, waits, small,
         minimum, maximum);
  return errors ? 5 : 0;
}
