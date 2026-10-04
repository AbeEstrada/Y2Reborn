/* SPDX-License-Identifier: CC0-1.0
 * Deterministic 4000x4000 artwork fixture, encoded by host FFmpeg libraries.
 * cc -O2 tools/build/jpeg_fixture.c -o /tmp/jpeg-fixture $(pkg-config --cflags --libs libavcodec libavutil)
 * /tmp/jpeg-fixture assets/fixtures/metadata/large.jpg
 */
#include <libavcodec/avcodec.h>
#include <libavutil/frame.h>
#include <stdio.h>
#include <string.h>
int main(int argc,char **argv){
 if(argc!=2)return 2;
 const AVCodec *codec=avcodec_find_encoder(AV_CODEC_ID_MJPEG);
 AVCodecContext *ctx=avcodec_alloc_context3(codec);AVFrame *frame=av_frame_alloc();AVPacket *p=av_packet_alloc();
 if(!ctx||!frame||!p)return 1;
 ctx->width=4000;ctx->height=4000;ctx->pix_fmt=AV_PIX_FMT_YUVJ420P;ctx->time_base=(AVRational){1,1};ctx->thread_count=1;
 if(avcodec_open2(ctx,codec,NULL)<0)return 1;
 frame->width=ctx->width;frame->height=ctx->height;frame->format=ctx->pix_fmt;
 if(av_frame_get_buffer(frame,32)<0)return 1;
 for(int plane=0;plane<3;plane++)for(int y=0;y<(plane?2000:4000);y++)memset(frame->data[plane]+y*frame->linesize[plane],plane==0?120:plane==1?100:80,plane?2000:4000);
 if(avcodec_send_frame(ctx,frame)<0||avcodec_receive_packet(ctx,p)<0)return 1;
 FILE *f=fopen(argv[1],"wb");if(!f)return 1;if(fwrite(p->data,1,p->size,f)!=(size_t)p->size)return 1;fclose(f);
 av_packet_free(&p);av_frame_free(&frame);avcodec_free_context(&ctx);return 0;
}
