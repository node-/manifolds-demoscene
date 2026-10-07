@echo off
rem Record the primary monitor (desktop duplication, 60 fps, NVENC) plus the
rem desktop audio mix (WASAPI loopback via tools\loopback-rec) to an MP4.
rem   scripts\record-demoscene.cmd <seconds> <out.mp4> [monitor_index]
rem Build the audio tool first:  cargo build --release -p loopback-rec
setlocal
set SECS=%1
set OUT=%2
set MON=%3
if "%SECS%"=="" set SECS=30
if "%OUT%"=="" set OUT=%USERPROFILE%\Videos\demoscene.mp4
if "%MON%"=="" set MON=0
set REPO=%~dp0..
set TARGET=%CARGO_TARGET_DIR%
if "%TARGET%"=="" set TARGET=%REPO%\target\windows-native
set REC=%TARGET%\release\loopback-rec.exe
set FFMPEG=ffmpeg.exe
rem Optional drawbox args to paint over a third-party overlay, e.g.
rem   set MASK=x=2080:y=0:w=480:h=84:color=0x03030D:t=fill
set VF=hwdownload,format=bgra,scale=2560:1440:flags=fast_bilinear
if defined MASK set VF=%VF%,drawbox=%MASK%
set VF=%VF%,format=yuv420p

"%REC%" %SECS% 48000 2 | "%FFMPEG%" -hide_banner -y ^
  -f lavfi -i "ddagrab=output_idx=%MON%:framerate=60:draw_mouse=0" ^
  -use_wallclock_as_timestamps 1 -f f32le -ar 48000 -ac 2 -i - ^
  -t %SECS% ^
  -vf "%VF%" ^
  -af "aresample=async=1:first_pts=0" ^
  -c:v h264_nvenc -preset p5 -tune hq -rc vbr -cq 19 -b:v 24M -maxrate 32M -bufsize 64M -profile:v high -g 120 -bf 2 ^
  -fps_mode cfr -r 60 ^
  -c:a aac -b:a 192k -ar 48000 ^
  -movflags +faststart "%OUT%"
endlocal
