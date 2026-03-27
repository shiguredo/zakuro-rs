# NopVideoDecoder (受信映像廃棄) を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

受信した映像をデコードせずに廃棄する NopVideoDecoder を実装し、受信側の CPU 負荷を削減する。

## 根拠

zakuro (C++) では NopVideoDecoder により受信映像のデコード処理をスキップしている。recvonly や sendrecv モードの負荷試験で、大量の仮想クライアントが映像を受信する際にデコーダの CPU 負荷がボトルネットになることを防ぐ。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. NopVideoDecoder の実装

- VideoDecoder トレイトを実装する
- デコード処理をスキップし、受信フレームを即座に廃棄する

### 2. デコーダ登録

- recvonly/sendrecv モードで NopVideoDecoder を使用する
