use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{ErrorMessage, Result};

/// YUV4MPEG2 (Y4M) 形式の動画ファイルリーダー
///
/// I420 (4:2:0) フォーマットのフレームを経過時間に基づいて読み出す。
/// ファイル終端に到達するとループ再生する。
pub(crate) struct Y4mReader {
    file: File,
    /// フレームデータ開始位置 (ヘッダ直後)
    start_pos: u64,
    /// 現在のファイル位置
    pos: u64,
    /// 現在のフレーム番号 (次に読むフレーム)
    frame: i64,
    /// 直前に返したフレーム番号
    prev_frame: i64,
    /// 映像幅
    width: i32,
    /// 映像高さ
    height: i32,
    /// フレームレート分子
    fps_num: i64,
    /// フレームレート分母
    fps_den: i64,
    /// ファイルサイズ
    file_size: u64,
}

impl Y4mReader {
    /// Y4M ファイルを開いてヘッダを解析する
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file_size = std::fs::metadata(path)
            .map_err(|e| ErrorMessage::new(format!("Y4M ファイルのメタデータ取得エラー: {e}")))?
            .len();

        let file = File::open(path)
            .map_err(|e| ErrorMessage::new(format!("Y4M ファイルのオープンエラー: {e}")))?;

        let mut reader = Self {
            file,
            start_pos: 0,
            pos: 0,
            frame: 0,
            prev_frame: -1,
            width: 0,
            height: 0,
            fps_num: 0,
            fps_den: 0,
            file_size,
        };
        reader.read_header()?;
        Ok(reader)
    }

    pub(crate) fn width(&self) -> i32 {
        self.width
    }

    pub(crate) fn height(&self) -> i32 {
        self.height
    }

    fn chroma_width(&self) -> i32 {
        (self.width + 1) / 2
    }

    fn chroma_height(&self) -> i32 {
        (self.height + 1) / 2
    }

    /// I420 フレーム 1 枚分のバイト数
    pub(crate) fn frame_size(&self) -> usize {
        let y = self.width as usize * self.height as usize;
        let uv = self.chroma_width() as usize * self.chroma_height() as usize * 2;
        y + uv
    }

    /// 経過時間 ms に対応するフレームを取得する
    ///
    /// 直前と同じフレームの場合は `Ok(None)` を返す。
    /// 新しいフレームの場合は I420 データを `buf` に書き込み `Ok(Some(()))` を返す。
    pub(crate) fn get_frame(&mut self, elapsed_ms: i64, buf: &mut [u8]) -> Result<Option<()>> {
        let frame = elapsed_ms * self.fps_num / (1000 * self.fps_den);

        // 直前と同じフレーム
        if self.prev_frame == frame {
            return Ok(None);
        }

        // 時間の巻き戻りは対応しない
        if frame < self.frame {
            return Err(ErrorMessage::new("Y4M: 時刻が巻き戻っています").into());
        }

        // 要求フレームまでスキップ
        while self.frame < frame {
            self.skip_frame()?;
        }

        // フレームヘッダを読む
        self.read_frame_header()?;

        // フレームデータを読む
        let size = self.frame_size();
        if buf.len() < size {
            return Err(ErrorMessage::new("Y4M: バッファが不足しています").into());
        }
        self.file
            .read_exact(&mut buf[..size])
            .map_err(|e| ErrorMessage::new(format!("Y4M: フレーム読み込みエラー: {e}")))?;
        self.pos += size as u64;

        if self.pos > self.file_size {
            return Err(ErrorMessage::new("Y4M: ファイル終端を超えて読み込みました").into());
        }

        // ファイル終端に到達したらループ
        if self.pos == self.file_size {
            self.seek_to_start()?;
        }

        self.frame += 1;
        self.prev_frame = frame;
        Ok(Some(()))
    }

    /// ヘッダ行を解析する
    fn read_header(&mut self) -> Result<()> {
        // ヘッダ行を読む (最大 1KB)
        let mut header_buf = [0u8; 1024];
        let n = self
            .file
            .read(&mut header_buf)
            .map_err(|e| ErrorMessage::new(format!("Y4M: ヘッダ読み込みエラー: {e}")))?;
        if n == 0 {
            return Err(ErrorMessage::new("Y4M: 空のファイルです").into());
        }

        let newline_pos = header_buf[..n]
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| ErrorMessage::new("Y4M: ヘッダ行が長すぎるか改行がありません"))?;

        let header = std::str::from_utf8(&header_buf[..newline_pos])
            .map_err(|_| ErrorMessage::new("Y4M: ヘッダが UTF-8 として不正です"))?;

        let mut tokens = header.split(' ');
        let signature = tokens.next().unwrap_or("");
        if signature != "YUV4MPEG2" {
            return Err(ErrorMessage::new("Y4M: シグネチャが不正です").into());
        }

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            let (tag, value) = token.split_at(1);
            match tag {
                "W" => {
                    self.width = value
                        .parse()
                        .map_err(|_| ErrorMessage::new("Y4M: 幅が不正です"))?;
                }
                "H" => {
                    self.height = value
                        .parse()
                        .map_err(|_| ErrorMessage::new("Y4M: 高さが不正です"))?;
                }
                "F" => {
                    let (num, den) = value
                        .split_once(':')
                        .ok_or_else(|| ErrorMessage::new("Y4M: フレームレートの形式が不正です"))?;
                    self.fps_num = num
                        .parse()
                        .map_err(|_| ErrorMessage::new("Y4M: フレームレート分子が不正です"))?;
                    self.fps_den = den
                        .parse()
                        .map_err(|_| ErrorMessage::new("Y4M: フレームレート分母が不正です"))?;
                }
                "I" => {
                    // プログレッシブのみ対応
                    if value != "p" {
                        return Err(ErrorMessage::new(
                            "Y4M: プログレッシブスキャン (Ip) のみ対応しています",
                        )
                        .into());
                    }
                }
                "A" => {
                    // アスペクト比は読み飛ばす (フレーム出力には不要)
                }
                "C" => {
                    // I420 系のみ対応
                    match value {
                        "420" | "420jpeg" | "420paldv" | "420mpeg2" => {}
                        _ => {
                            return Err(ErrorMessage::new(format!(
                                "Y4M: 未対応の色差フォーマット: C{value}"
                            ))
                            .into());
                        }
                    }
                }
                "X" => {
                    // 拡張タグは無視
                }
                _ => {
                    return Err(
                        ErrorMessage::new(format!("Y4M: 未知のヘッダトークン: {token}")).into(),
                    );
                }
            }
        }

        if self.width <= 0 || self.height <= 0 || self.fps_num <= 0 {
            return Err(
                ErrorMessage::new("Y4M: 必須ヘッダフィールド (W, H, F) が不足しています").into(),
            );
        }
        if self.fps_den <= 0 {
            return Err(
                ErrorMessage::new("Y4M: フレームレート分母は正の値である必要があります").into(),
            );
        }

        let data_start = (newline_pos + 1) as u64;
        self.file
            .seek(SeekFrom::Start(data_start))
            .map_err(|e| ErrorMessage::new(format!("Y4M: シークエラー: {e}")))?;
        self.start_pos = data_start;
        self.pos = data_start;
        self.frame = 0;
        self.prev_frame = -1;

        Ok(())
    }

    /// FRAME ヘッダを読み飛ばす
    fn read_frame_header(&mut self) -> Result<()> {
        let mut tag = [0u8; 5];
        self.file
            .read_exact(&mut tag)
            .map_err(|e| ErrorMessage::new(format!("Y4M: フレームヘッダ読み込みエラー: {e}")))?;
        if &tag != b"FRAME" {
            return Err(ErrorMessage::new("Y4M: FRAME タグが必要です").into());
        }
        self.pos += 5;

        // '\n' まで読み飛ばす (最大 1KB)
        for _ in 0..1024 {
            let mut byte = [0u8; 1];
            self.file.read_exact(&mut byte).map_err(|e| {
                ErrorMessage::new(format!("Y4M: フレームヘッダ読み込みエラー: {e}"))
            })?;
            self.pos += 1;
            if byte[0] == b'\n' {
                return Ok(());
            }
        }

        Err(ErrorMessage::new("Y4M: フレームヘッダが長すぎます").into())
    }

    /// 1 フレーム分をスキップする
    fn skip_frame(&mut self) -> Result<()> {
        self.read_frame_header()?;

        let size = self.frame_size() as u64;
        self.file
            .seek(SeekFrom::Current(size as i64))
            .map_err(|e| ErrorMessage::new(format!("Y4M: シークエラー: {e}")))?;
        self.pos += size;

        if self.pos > self.file_size {
            return Err(ErrorMessage::new("Y4M: ファイル終端を超えてシークしました").into());
        }

        // ファイル終端に到達したらループ
        if self.pos == self.file_size {
            self.seek_to_start()?;
        }

        self.frame += 1;
        Ok(())
    }

    /// フレームデータ開始位置にシークする
    fn seek_to_start(&mut self) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(self.start_pos))
            .map_err(|e| ErrorMessage::new(format!("Y4M: シークエラー: {e}")))?;
        self.pos = self.start_pos;
        Ok(())
    }
}
