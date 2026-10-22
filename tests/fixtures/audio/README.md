`silence.mp3` contains 80 ms of generated silence (mono, 22050 Hz).
It tests real MP3 decoding and duration without redistributing project audio.

Regenerate with FFmpeg:

```sh
ffmpeg -f lavfi -i anullsrc=r=22050:cl=mono -t 0.08 -c:a libmp3lame -b:a 32k silence.mp3
```
