# testdata

`src/mp4_audio.rs` の単体テストが使う MP4 / M4A フィクスチャ。すべて ffmpeg で生成する。

## 生成コマンド

各フィクスチャのテストはエンコード設定 (コーデック・チャンネル数・サンプルレート・
パケット長) に依存するため、再生成時は以下のコマンドをそのまま使うこと。

```bash
# 映像のみ (音声トラック無し)
ffmpeg -y -loglevel error -f lavfi -i "color=c=red:s=160x120:d=2" -c:v mpeg4 mp4-video-only.mp4

# 映像 + Opus ステレオ音声 (複合)
ffmpeg -y -loglevel error -f lavfi -i "color=c=red:s=160x120:d=2" \
  -f lavfi -i "sine=frequency=1000:duration=2:sample_rate=48000" \
  -c:v mpeg4 -c:a libopus -b:a 64k -ac 2 -shortest mp4-video-with-opus-audio.mp4

# Opus ステレオ (48kHz・2 秒・20ms パケット)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=2:sample_rate=48000" \
  -c:a libopus -b:a 64k -ac 2 mp4-audio-opus-stereo.mp4

# Opus モノラル (48kHz・2 秒)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=2:sample_rate=48000" \
  -c:a libopus -b:a 64k -ac 1 mp4-audio-opus-mono.mp4

# AAC-LC ステレオ (44.1kHz・4 秒)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=4:sample_rate=44100" \
  -c:a aac -b:a 128k -ac 2 mp4-audio-aac-stereo.m4a

# 音声トラック 2 本 (Opus + AAC)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=2:sample_rate=48000" \
  -f lavfi -i "sine=frequency=500:duration=2:sample_rate=48000" \
  -map 0:a -map 1:a -c:a:0 libopus -c:a:1 aac mp4-audio-two-tracks.mp4

# FLAC 音声 (未対応コーデック)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=2:sample_rate=48000" \
  -c:a flac mp4-audio-flac.mp4

# 6 チャンネル AAC (チャンネル構成非対応)
ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=1000:duration=1:sample_rate=48000" \
  -ac 6 -c:a aac mp4-audio-6ch.m4a
```

## テストが依存するエンコード設定

- Opus フィクスチャは 20ms パケット (48kHz で 960 サンプル) を前提とする
  (`mp4_audio.rs` のテスト期待値に 960 が使われる)。libopus のデフォルト
  (20ms) で生成される
- AAC フィクスチャは AAC-LC・44.1kHz を前提とする (1 フレーム 1024 サンプルが
  48kHz モノラルへリサンプリングされ 1114 サンプルになる期待値)