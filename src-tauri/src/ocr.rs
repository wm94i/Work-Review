// OCR 文字识别模块
// Windows: 使用 PaddleOCR (轻量化版本)
// macOS: 使用 Vision 框架

use image::{imageops::FilterType, DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Output;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Instant;
use uuid::Uuid;
use work_review_core::error::{AppError, Result};

const OCR_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const PADDLE_WORKER_READY_TIMEOUT: Duration = Duration::from_secs(45);

/// OCR 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub confidence: f32,
    pub boxes: Vec<OcrBox>,
}

/// OCR 文字框
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBox {
    pub text: String,
    pub confidence: f32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// OCR 引擎类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OcrEngine {
    /// PaddleOCR (跨平台，需要预先安装，预留功能)
    #[allow(dead_code)]
    PaddleOCR,
    /// macOS Vision 框架
    #[cfg(target_os = "macos")]
    MacOSVision,
    /// Windows OCR (通过 PowerShell)
    #[cfg(target_os = "windows")]
    WindowsOCR,
    /// Tesseract OCR (Linux)
    #[cfg(target_os = "linux")]
    Tesseract,
}

/// OCR 服务
pub struct OcrService {
    /// 首选引擎
    preferred_engine: OcrEngine,
    /// PaddleOCR Python 脚本路径
    paddle_script_path: Option<std::path::PathBuf>,
}

static PADDLE_WORKER: OnceLock<Mutex<Option<PaddleWorkerClient>>> = OnceLock::new();

#[cfg(target_os = "windows")]
static WINDOWS_OCR_WORKER: OnceLock<Mutex<Option<WindowsOcrWorker>>> = OnceLock::new();

#[cfg(target_os = "windows")]
const WINDOWS_OCR_WORKER_READY_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(target_os = "windows")]
struct WindowsOcrWorker {
    child: Child,
    stdin: ChildStdin,
    response_rx: mpsc::Receiver<String>,
    script_path: PathBuf,
}

#[cfg(target_os = "windows")]
impl Drop for WindowsOcrWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.script_path);
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_OCR_WORKER_SCRIPT: &str = r#"
$utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::InputEncoding = $utf8
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8

Add-Type -AssemblyName System.Runtime.WindowsRuntime

function Write-OcrJson($payload) {
    [Console]::Out.WriteLine((ConvertTo-Json $payload -Depth 5 -Compress))
    [Console]::Out.Flush()
}

function Write-OcrError([string]$message) {
    Write-OcrJson @{
        text = ""
        error = ([string]$message)
        boxes = @()
        confidence = 0
    }
}

[Windows.Media.Ocr.OcrEngine, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapTransform, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null
[Windows.Globalization.Language, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null
[Windows.System.UserProfile.GlobalizationPreferences, Windows.Foundation.UniversalApiContract, ContentType = WindowsRuntime] | Out-Null

$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
})[0]
if ($null -eq $asTaskGeneric) {
    Write-OcrError "System.WindowsRuntimeSystemExtensions.AsTask 未找到"
    exit 1
}

function Await($WinRtTask, $ResultType) {
    if ($null -eq $WinRtTask) {
        throw "WinRT 异步任务为空: $ResultType"
    }
    $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
    $netTask = $asTask.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
    $netTask.Result
}

$ocrEngine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $ocrEngine) {
    foreach ($langTag in @('zh-Hans', 'zh-Hans-CN', 'en-US')) {
        try {
            $language = [Windows.Globalization.Language]::new($langTag)
            $candidate = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage($language)
            if ($null -ne $candidate) {
                $ocrEngine = $candidate
                break
            }
        } catch {
        }
    }
}

if ($null -eq $ocrEngine) {
    $profileLanguages = [string]::Join(',', [Windows.System.UserProfile.GlobalizationPreferences]::Languages)
    Write-OcrError ("No OCR engine available; user profile languages=" + ([string]$profileLanguages))
    exit 1
}

$maxDimension = [Windows.Media.Ocr.OcrEngine]::MaxImageDimension
[Console]::Out.WriteLine("READY")
[Console]::Out.Flush()

while ($null -ne ($encodedLine = [Console]::In.ReadLine())) {
    $trimmed = $encodedLine.Trim()
    if ([string]::IsNullOrEmpty($trimmed)) {
        continue
    }
    if ($trimmed -eq "__QUIT__") {
        break
    }

    $stream = $null
    $bitmap = $null
    $ocrBitmap = $null
    try {
        $imagePath = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($trimmed))
        $file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($imagePath)) ([Windows.Storage.StorageFile])
        if ($null -eq $file) {
            throw "读取截图文件失败: $imagePath"
        }
        $stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
        if ($null -eq $stream) {
            throw "打开截图流失败: $imagePath"
        }

        $decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
        $sourceWidth = $decoder.PixelWidth
        $sourceHeight = $decoder.PixelHeight
        if ([Math]::Max($sourceWidth, $sourceHeight) -gt $maxDimension) {
            $scale = [double]$maxDimension / [Math]::Max($sourceWidth, $sourceHeight)
            $transform = [Windows.Graphics.Imaging.BitmapTransform]::new()
            $transform.ScaledWidth = [uint32][Math]::Max(1, [Math]::Floor($sourceWidth * $scale))
            $transform.ScaledHeight = [uint32][Math]::Max(1, [Math]::Floor($sourceHeight * $scale))
            $bitmap = Await ($decoder.GetSoftwareBitmapAsync(
                [Windows.Graphics.Imaging.BitmapPixelFormat]::Bgra8,
                [Windows.Graphics.Imaging.BitmapAlphaMode]::Premultiplied,
                $transform,
                [Windows.Graphics.Imaging.ExifOrientationMode]::IgnoreExifOrientation,
                [Windows.Graphics.Imaging.ColorManagementMode]::DoNotColorManage
            )) ([Windows.Graphics.Imaging.SoftwareBitmap])
        } else {
            $bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
        }
        if ($null -eq $bitmap) {
            throw "解码截图失败: $imagePath"
        }

        $ocrBitmap = [Windows.Graphics.Imaging.SoftwareBitmap]::Convert(
            $bitmap,
            [Windows.Graphics.Imaging.BitmapPixelFormat]::Bgra8,
            [Windows.Graphics.Imaging.BitmapAlphaMode]::Premultiplied
        )
        if ($null -eq $ocrBitmap) {
            throw "转换 OCR 位图失败: $imagePath"
        }

        $result = Await ($ocrEngine.RecognizeAsync($ocrBitmap)) ([Windows.Media.Ocr.OcrResult])
        if ($null -eq $result) {
            throw "OCR 结果为空"
        }

        $lines = New-Object System.Collections.Generic.List[string]
        foreach ($line in $result.Lines) {
            [void]$lines.Add($line.Text)
        }

        Write-OcrJson @{
            text = [string]::Join("`n", $lines)
            boxes = @()
            confidence = 0.9
        }
    } catch {
        $message = if ($_.Exception -and $_.Exception.Message) {
            [string]$_.Exception.Message
        } else {
            [string]$_
        }
        Write-OcrError $message
    } finally {
        if ($null -ne $ocrBitmap) { try { $ocrBitmap.Dispose() } catch {} }
        if ($null -ne $bitmap) { try { $bitmap.Dispose() } catch {} }
        if ($null -ne $stream) { try { $stream.Dispose() } catch {} }
    }
}
"#;

#[cfg(target_os = "windows")]
impl WindowsOcrWorker {
    fn spawn() -> Result<Self> {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let powershell_path =
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
        let script_path = std::env::temp_dir().join(format!(
            "work_review_winrt_ocr_worker_{}.ps1",
            Uuid::new_v4().simple()
        ));

        let bom: &[u8] = b"\xEF\xBB\xBF";
        let mut content = Vec::with_capacity(bom.len() + WINDOWS_OCR_WORKER_SCRIPT.len());
        content.extend_from_slice(bom);
        content.extend_from_slice(WINDOWS_OCR_WORKER_SCRIPT.as_bytes());
        std::fs::write(&script_path, &content)?;

        let mut child = match Command::new(&powershell_path)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Sta",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                script_path.to_string_lossy().as_ref(),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&script_path);
                return Err(AppError::Unknown(format!(
                    "启动 Windows OCR 常驻进程失败: {error}"
                )));
            }
        };

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Unknown("无法打开 Windows OCR worker stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Unknown("无法打开 Windows OCR worker stdout".to_string()))?;
        let stderr = child.stderr.take();

        if let Some(stderr) = stderr {
            thread::Builder::new()
                .name("work-review-winrt-ocr-stderr".to_string())
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(std::result::Result::ok) {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            log::debug!("Windows OCR worker stderr: {trimmed}");
                        }
                    }
                })
                .ok();
        }

        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("work-review-winrt-ocr-stdout".to_string())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(std::result::Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .ok();

        let worker = Self {
            child,
            stdin,
            response_rx: rx,
            script_path,
        };

        match worker
            .response_rx
            .recv_timeout(WINDOWS_OCR_WORKER_READY_TIMEOUT)
        {
            Ok(line) if line.trim() == "READY" => {
                log::info!("Windows WinRT OCR 常驻进程已就绪");
                Ok(worker)
            }
            Ok(line) => Err(AppError::Unknown(format!(
                "Windows OCR worker 初始化返回异常: {}",
                line.trim()
            ))),
            Err(error) => Err(AppError::Unknown(format!(
                "等待 Windows OCR worker 就绪超时或断开: {error}"
            ))),
        }
    }

    fn recognize(&mut self, image_path: &Path) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let encoded = BASE64_STANDARD.encode(image_path.to_string_lossy().as_bytes());
        writeln!(self.stdin, "{encoded}")?;
        self.stdin.flush()?;
        self.response_rx
            .recv_timeout(OCR_COMMAND_TIMEOUT)
            .map_err(|e| AppError::Unknown(format!("Windows OCR worker 响应超时或断开: {e}")))
    }
}

#[cfg(target_os = "windows")]
fn request_windows_ocr_via_worker(image_path: &Path) -> Result<String> {
    let lock = WINDOWS_OCR_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    // 最多尝试一次现有进程 + 一次重启后重试
    for attempt in 0..2 {
        if guard.is_none() {
            *guard = Some(WindowsOcrWorker::spawn()?);
        }
        let result = guard
            .as_mut()
            .expect("Windows OCR worker 已初始化")
            .recognize(image_path);
        match result {
            Ok(line) => return Ok(line),
            Err(error) => {
                log::warn!(
                    "Windows OCR worker 请求失败 (attempt {}): {error}，准备重建 worker",
                    attempt + 1
                );
                *guard = None;
                if attempt == 1 {
                    return Err(error);
                }
            }
        }
    }

    Err(AppError::Unknown("Windows OCR worker 调用失败".to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct PaddleModelConfig {
    det_model_name: Option<String>,
    rec_model_name: Option<String>,
    ocr_version: Option<String>,
}

struct PaddleWorkerClient {
    python_cmd: String,
    script_path: PathBuf,
    model_config: PaddleModelConfig,
    child: Child,
    stdin: ChildStdin,
    response_rx: mpsc::Receiver<String>,
}

impl OcrService {
    /// 创建 OCR 服务
    pub fn new(data_dir: &Path) -> Self {
        let paddle_script_path = data_dir.join("paddle_ocr.py");

        // 始终校验脚本内容，确保老版本脚本会被自动升级到当前协议。
        let _ = Self::ensure_paddle_script(&paddle_script_path);

        // 选择默认引擎（优先使用系统原生 OCR，无需 Python 依赖）
        #[cfg(target_os = "windows")]
        let preferred_engine = OcrEngine::WindowsOCR;

        #[cfg(target_os = "macos")]
        let preferred_engine = OcrEngine::MacOSVision;

        #[cfg(target_os = "linux")]
        let preferred_engine = OcrEngine::Tesseract;

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let preferred_engine = OcrEngine::PaddleOCR;

        Self {
            preferred_engine,
            paddle_script_path: Some(paddle_script_path),
        }
    }

    fn paddle_script_source() -> &'static str {
        r#"#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
PaddleOCR 调用脚本 (兼容 PaddleOCR 3.x)
"""

import sys
import json
import os
import warnings
import contextlib
warnings.filterwarnings('ignore')

# 禁用模型源检查以加速初始化
os.environ['DISABLE_MODEL_SOURCE_CHECK'] = 'True'
os.environ.setdefault('PYTHONUNBUFFERED', '1')

def emit(payload):
    print(json.dumps(payload, ensure_ascii=False), flush=True)

def read_model_config():
    config = {}
    det_model_name = os.environ.get('WORK_REVIEW_PADDLE_DET_MODEL_NAME', '').strip()
    rec_model_name = os.environ.get('WORK_REVIEW_PADDLE_REC_MODEL_NAME', '').strip()
    ocr_version = os.environ.get('WORK_REVIEW_PADDLE_OCR_VERSION', '').strip()

    if det_model_name:
        config['text_detection_model_name'] = det_model_name
    if rec_model_name:
        config['text_recognition_model_name'] = rec_model_name
    if ocr_version:
        config['ocr_version'] = ocr_version

    return config

def build_ocr():
    from paddleocr import PaddleOCR
    ocr_kwargs = {
        'use_doc_orientation_classify': False,
        'use_doc_unwarping': False,
        'use_textline_orientation': False,
    }
    ocr_kwargs.update(read_model_config())

    with contextlib.redirect_stdout(sys.stderr), contextlib.redirect_stderr(sys.stderr):
        return PaddleOCR(**ocr_kwargs)

def recognize(ocr, image_path):
    try:
        with contextlib.redirect_stdout(sys.stderr), contextlib.redirect_stderr(sys.stderr):
            result = ocr.predict(input=image_path)

        if result is None:
            return {"text": "", "boxes": [], "confidence": 0}

        boxes = []
        all_text = []
        total_conf = 0
        count = 0

        for res in result:
            rec_texts = res.get('rec_texts', []) if hasattr(res, 'get') else getattr(res, 'rec_texts', [])
            rec_scores = res.get('rec_scores', []) if hasattr(res, 'get') else getattr(res, 'rec_scores', [])
            rec_boxes = res.get('dt_polys', []) if hasattr(res, 'get') else getattr(res, 'dt_polys', [])
            if rec_texts and rec_scores:
                for i, (text, conf) in enumerate(zip(rec_texts, rec_scores)):
                    box_info = {
                        "text": text,
                        "confidence": float(conf),
                        "x": 0,
                        "y": 0,
                        "width": 0,
                        "height": 0
                    }
                    # 尝试获取边界框
                    if i < len(rec_boxes):
                        box = rec_boxes[i]
                        x_coords = [p[0] for p in box]
                        y_coords = [p[1] for p in box]
                        box_info["x"] = int(min(x_coords))
                        box_info["y"] = int(min(y_coords))
                        box_info["width"] = int(max(x_coords) - box_info["x"])
                        box_info["height"] = int(max(y_coords) - box_info["y"])

                    boxes.append(box_info)
                    all_text.append(text)
                    total_conf += float(conf)
                    count += 1

        avg_conf = total_conf / count if count > 0 else 0

        return {
            "text": "\n".join(all_text),
            "boxes": boxes,
            "confidence": avg_conf
        }

    except ImportError:
        return {
            "error": "PaddleOCR not installed. Install with: pip install paddlepaddle paddleocr",
            "text": "",
            "boxes": [],
            "confidence": 0
        }
    except Exception as e:
        return {
            "error": str(e),
            "text": "",
            "boxes": [],
            "confidence": 0
        }

def run_once(image_path):
    ocr = build_ocr()
    emit(recognize(ocr, image_path))

def run_worker():
    ocr = build_ocr()
    emit({"status": "ready"})

    for line in sys.stdin:
        raw = line.strip()
        if not raw:
            continue
        try:
            request = json.loads(raw)
        except Exception as e:
            emit({
                "error": f"invalid request: {e}",
                "text": "",
                "boxes": [],
                "confidence": 0
            })
            continue

        if request.get("command") == "shutdown":
            break

        image_path = request.get("image_path", "")
        emit(recognize(ocr, image_path))

if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--worker":
        run_worker()
        sys.exit(0)

    if len(sys.argv) < 2:
        emit({"error": "No image path provided", "text": "", "boxes": [], "confidence": 0})
        sys.exit(1)

    run_once(sys.argv[1])
"#
    }

    /// 创建 PaddleOCR Python 脚本
    fn create_paddle_script(script_path: &Path) -> Result<()> {
        std::fs::write(script_path, Self::paddle_script_source())?;
        Ok(())
    }

    fn ensure_paddle_script(script_path: &Path) -> Result<()> {
        match std::fs::read_to_string(script_path) {
            Ok(existing) if existing == Self::paddle_script_source() => Ok(()),
            Ok(_) => Self::create_paddle_script(script_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::create_paddle_script(script_path)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// 创建 PaddleOCR Python 脚本
    fn create_or_refresh_paddle_script(script_path: &Path) -> Result<()> {
        Self::ensure_paddle_script(script_path)?;
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn run_command_with_timeout(command: &mut Command, context: &str) -> Result<Output> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| AppError::Unknown(format!("{context} 启动失败: {e}")))?;

        // 起线程持续排空 stdout/stderr 管道，避免子进程因管道缓冲（默认 64KB）写满
        // 而阻塞在写操作上——否则 try_wait 会一直返回 None 直到超时，OCR 对大图/密集
        // 文本输出（常见）会必现超时失败、丢掉全部结果。
        let stdout_handle = child.stdout.take().map(|mut s| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut s, &mut buf);
                buf
            })
        });
        let stderr_handle = child.stderr.take().map(|mut s| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut s, &mut buf);
                buf
            })
        });

        let started_at = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started_at.elapsed() < OCR_COMMAND_TIMEOUT => {
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::Unknown(format!(
                        "{context} 执行超时（>{}s）",
                        OCR_COMMAND_TIMEOUT.as_secs()
                    )));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::Unknown(format!("{context} 等待进程失败: {e}")));
                }
            }
        };

        // 子进程已退出（或被 kill 后管道关闭），读线程结束，join 取回输出。
        let stdout = stdout_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stderr = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    /// 获取 Python 路径（优先使用 work_review conda 环境）
    fn get_python_path() -> String {
        if let Ok(override_python) = std::env::var("WORK_REVIEW_PADDLE_PYTHON") {
            let trimmed = override_python.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        // 优先检查 work_review conda 环境
        let conda_python = if cfg!(target_os = "windows") {
            std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                .join("miniconda3")
                .join("envs")
                .join("work_review")
                .join("python.exe")
        } else {
            // macOS / Linux
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(&home)
                .join("all_environments")
                .join("miniconda3")
                .join("envs")
                .join("work_review")
                .join("bin")
                .join("python")
        };

        if conda_python.exists() {
            return conda_python.to_string_lossy().to_string();
        }

        // 备用：检查标准 conda 路径
        let std_conda_python = if cfg!(target_os = "windows") {
            std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                .join("anaconda3")
                .join("envs")
                .join("work_review")
                .join("python.exe")
        } else {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(&home)
                .join("miniconda3")
                .join("envs")
                .join("work_review")
                .join("bin")
                .join("python")
        };

        if std_conda_python.exists() {
            return std_conda_python.to_string_lossy().to_string();
        }

        // 最后使用系统 python
        "python3".to_string()
    }

    fn get_paddle_model_config() -> PaddleModelConfig {
        PaddleModelConfig::from_env()
    }

    /// 从图片中提取文字
    pub fn extract_text(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        // Windows 原生 OCR 每次识别都会启动 PowerShell；区域补救会把单次任务放大为 4 个进程。
        #[cfg(target_os = "windows")]
        if self.preferred_engine == OcrEngine::WindowsOCR {
            return self.extract_text_once(image_path);
        }

        execute_ocr_pipeline_with(image_path, |path| self.extract_text_once(path))
    }

    /// 使用当前首选引擎执行单次 OCR
    fn extract_text_once(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        match self.preferred_engine {
            OcrEngine::PaddleOCR => self.extract_with_paddle(image_path),
            #[cfg(target_os = "macos")]
            OcrEngine::MacOSVision => self.extract_with_vision(image_path),
            #[cfg(target_os = "windows")]
            OcrEngine::WindowsOCR => self.extract_with_windows_ocr(image_path),
            #[cfg(target_os = "linux")]
            OcrEngine::Tesseract => self.extract_with_tesseract(image_path),
        }
    }

    /// 使用 PaddleOCR 提取文字
    fn extract_with_paddle(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        let script_path = match &self.paddle_script_path {
            Some(p) => p,
            None => return Ok(None),
        };

        Self::create_or_refresh_paddle_script(script_path)?;

        // 获取 Python 路径（优先使用 work_review conda 环境）
        let python_cmd = Self::get_python_path();
        let model_config = Self::get_paddle_model_config();

        match request_paddle_via_worker(&python_cmd, script_path, image_path, &model_config) {
            Ok(result) => Ok(result),
            Err(e) => {
                log::warn!("PaddleOCR worker 启动失败: {e}，尝试回退方案");

                #[cfg(target_os = "windows")]
                return self.extract_with_windows_ocr(image_path);
                #[cfg(not(target_os = "windows"))]
                Ok(None)
            }
        }
    }

    /// 使用 Windows OCR API（通过常驻 PowerShell + WinRT worker，避免每张截图冷启动进程）
    #[cfg(target_os = "windows")]
    fn extract_with_windows_ocr(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        match request_windows_ocr_via_worker(image_path) {
            Ok(stdout) => {
                if let Ok(ocr_output) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if let Some(error) = ocr_output.get("error").and_then(|v| v.as_str()) {
                        log::warn!("Windows OCR 错误: {error}");
                        return Ok(None);
                    }

                    let text = ocr_output
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if text.is_empty() {
                        return Ok(None);
                    }

                    let confidence = ocr_output
                        .get("confidence")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.9) as f32;

                    let boxes: Vec<OcrBox> = ocr_output
                        .get("boxes")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();

                    log::debug!("Windows OCR 识别到 {} 个字符", text.len());

                    Ok(Some(OcrResult {
                        text,
                        confidence,
                        boxes,
                    }))
                } else {
                    if !stdout.trim().is_empty() {
                        log::warn!("Windows OCR 输出无法解析为 JSON: {}", stdout.trim());
                    }
                    Ok(None)
                }
            }
            Err(e) => {
                log::warn!("Windows OCR worker 执行失败: {e}");
                Ok(None)
            }
        }
    }

    /// 使用 macOS Vision 框架提取文字
    #[cfg(target_os = "macos")]
    fn extract_with_vision(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        let script = format!(
            r#"
use framework "Vision"
use framework "Foundation"
use scripting additions

set imagePath to "{}"
set theImage to current application's NSImage's alloc()'s initWithContentsOfFile:imagePath

if theImage is missing value then
    return ""
end if

set requestHandler to current application's VNImageRequestHandler's alloc()'s initWithData:(theImage's TIFFRepresentation()) options:(current application's NSDictionary's dictionary())
set theRequest to current application's VNRecognizeTextRequest's alloc()'s init()
theRequest's setRecognitionLevel:(current application's VNRequestTextRecognitionLevelAccurate)
theRequest's setRecognitionLanguages:{{"zh-Hans", "en"}}

requestHandler's performRequests:{{theRequest}} |error|:(missing value)

set theResults to theRequest's results()
set outputText to ""

repeat with observation in theResults
    set outputText to outputText & (observation's topCandidates:1)'s firstObject()'s |string|() & linefeed
end repeat

return outputText
            "#,
            image_path.to_string_lossy()
        );

        let output = Self::run_command_with_timeout(
            Command::new("osascript")
                .arg("-l")
                .arg("AppleScript")
                .arg("-e")
                .arg(&script),
            "Vision OCR",
        );

        match output {
            Ok(result) if result.status.success() => {
                let text = String::from_utf8_lossy(&result.stdout).trim().to_string();
                if text.is_empty() {
                    Ok(None)
                } else {
                    log::debug!("Vision OCR 识别到 {} 个字符", text.len());
                    Ok(Some(OcrResult {
                        text,
                        confidence: 0.9,
                        boxes: vec![],
                    }))
                }
            }
            Ok(result) => {
                log::warn!(
                    "Vision OCR 命令执行失败: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                Ok(None)
            }
            Err(e) => {
                log::warn!("Vision OCR 执行错误: {e}");
                Ok(None)
            }
        }
    }

    /// 使用 Tesseract OCR 提取文字 (Linux)
    #[cfg(target_os = "linux")]
    fn extract_with_tesseract(&self, image_path: &Path) -> Result<Option<OcrResult>> {
        // 使用 tesseract 命令行工具，支持中英文混合辨识
        let output = Self::run_command_with_timeout(
            Command::new("tesseract")
                .arg(image_path)
                .arg("stdout")
                .args(["-l", "chi_tra+chi_sim+eng"])
                .args(["--psm", "3"]),
            "Tesseract OCR",
        );

        match output {
            Ok(result) if result.status.success() => {
                let text = String::from_utf8_lossy(&result.stdout).trim().to_string();
                if text.is_empty() {
                    Ok(None)
                } else {
                    log::debug!("Tesseract OCR 识别到 {} 个字符", text.len());
                    Ok(Some(OcrResult {
                        text,
                        confidence: 0.85,
                        boxes: vec![],
                    }))
                }
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                log::warn!("Tesseract OCR 执行失败: {stderr}");
                // 降级到 PaddleOCR
                self.extract_with_paddle(image_path)
            }
            Err(e) => {
                log::warn!("Tesseract OCR 启动失败: {e}，尝试 PaddleOCR");
                self.extract_with_paddle(image_path)
            }
        }
    }
}

impl PaddleWorkerClient {
    fn start(
        python_cmd: &str,
        script_path: &Path,
        model_config: &PaddleModelConfig,
    ) -> Result<Self> {
        let mut command = Command::new(python_cmd);
        command
            .arg("-u")
            .arg(script_path)
            .arg("--worker")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        model_config.apply_to_command(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| AppError::Unknown(format!("PaddleOCR worker 启动失败: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Unknown("PaddleOCR worker stdin 不可用".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Unknown("PaddleOCR worker stdout 不可用".to_string()))?;

        let response_rx = spawn_worker_stdout_reader(stdout);
        if let Some(stderr) = child.stderr.take() {
            spawn_worker_stderr_logger(stderr);
        }

        let mut worker = Self {
            python_cmd: python_cmd.to_string(),
            script_path: script_path.to_path_buf(),
            model_config: model_config.clone(),
            child,
            stdin,
            response_rx,
        };
        worker.wait_for_ready()?;
        Ok(worker)
    }

    fn matches_config(
        &self,
        python_cmd: &str,
        script_path: &Path,
        model_config: &PaddleModelConfig,
    ) -> bool {
        self.python_cmd == python_cmd
            && self.script_path == script_path
            && self.model_config == *model_config
    }

    fn request(&mut self, image_path: &Path) -> Result<Option<OcrResult>> {
        self.ensure_running("PaddleOCR worker 请求前检查")?;

        let request = serde_json::json!({
            "image_path": image_path.to_string_lossy().to_string(),
        });
        self.send_json_line(&request, "PaddleOCR worker 写入请求")?;
        let line = self.recv_response_line(OCR_COMMAND_TIMEOUT, "PaddleOCR worker 响应超时")?;
        parse_paddle_response_line(&line)
    }

    fn wait_for_ready(&mut self) -> Result<()> {
        let line =
            self.recv_response_line(PADDLE_WORKER_READY_TIMEOUT, "PaddleOCR worker 启动超时")?;
        let value: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| AppError::Unknown(format!("PaddleOCR worker ready 输出不是 JSON: {e}")))?;

        if value.get("status").and_then(|v| v.as_str()) == Some("ready") {
            Ok(())
        } else {
            Err(AppError::Unknown(format!(
                "PaddleOCR worker 未返回 ready: {line}"
            )))
        }
    }

    fn recv_response_line(&mut self, timeout: Duration, context: &str) -> Result<String> {
        self.response_rx.recv_timeout(timeout).map_err(|e| {
            AppError::Unknown(format!(
                "{context}: {}",
                match e {
                    mpsc::RecvTimeoutError::Timeout => "等待超时".to_string(),
                    mpsc::RecvTimeoutError::Disconnected => "worker 输出通道已断开".to_string(),
                }
            ))
        })
    }

    fn send_json_line(&mut self, value: &serde_json::Value, context: &str) -> Result<()> {
        let payload = serde_json::to_string(value)
            .map_err(|e| AppError::Unknown(format!("{context}: JSON 序列化失败: {e}")))?;

        writeln!(self.stdin, "{payload}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| AppError::Unknown(format!("{context}: {e}")))
    }

    fn ensure_running(&mut self, context: &str) -> Result<()> {
        match self.child.try_wait() {
            Ok(Some(status)) => Err(AppError::Unknown(format!(
                "{context}: worker 已退出，状态码 {:?}",
                status.code()
            ))),
            Ok(None) => Ok(()),
            Err(e) => Err(AppError::Unknown(format!(
                "{context}: 检查进程状态失败: {e}"
            ))),
        }
    }

    fn shutdown(&mut self) {
        let _ = self.send_json_line(
            &serde_json::json!({"command": "shutdown"}),
            "PaddleOCR worker 关闭",
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PaddleWorkerClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_worker_stdout_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    log::debug!("PaddleOCR worker stdout 读取失败: {error}");
                    break;
                }
            }
        }
    });
    rx
}

fn spawn_worker_stderr_logger(stderr: ChildStderr) {
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    log::debug!("PaddleOCR worker stderr: {}", line.trim());
                }
                Ok(_) => {}
                Err(error) => {
                    log::debug!("PaddleOCR worker stderr 读取失败: {error}");
                    break;
                }
            }
        }
    });
}

fn paddle_worker_slot() -> &'static Mutex<Option<PaddleWorkerClient>> {
    PADDLE_WORKER.get_or_init(|| Mutex::new(None))
}

fn request_paddle_via_worker(
    python_cmd: &str,
    script_path: &Path,
    image_path: &Path,
    model_config: &PaddleModelConfig,
) -> Result<Option<OcrResult>> {
    let mut last_error = None;

    for attempt in 0..2 {
        let mut slot = paddle_worker_slot()
            .lock()
            .map_err(|e| AppError::Unknown(format!("PaddleOCR worker 锁定失败: {e}")))?;

        let worker = ensure_paddle_worker(&mut slot, python_cmd, script_path, model_config)?;
        match worker.request(image_path) {
            Ok(result) => return Ok(result),
            Err(error) => {
                log::warn!("PaddleOCR worker 请求失败: {error}");
                if let Some(existing) = slot.as_mut() {
                    existing.shutdown();
                }
                *slot = None;
                last_error = Some(error);
                if attempt == 0 {
                    log::info!("PaddleOCR worker 将自动重启一次并重试");
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| AppError::Unknown("PaddleOCR worker 未返回结果".to_string())))
}

fn ensure_paddle_worker<'a>(
    slot: &'a mut Option<PaddleWorkerClient>,
    python_cmd: &str,
    script_path: &Path,
    model_config: &PaddleModelConfig,
) -> Result<&'a mut PaddleWorkerClient> {
    let needs_restart = match slot.as_mut() {
        Some(worker) => {
            !worker.matches_config(python_cmd, script_path, model_config)
                || worker.ensure_running("PaddleOCR worker 复用检查").is_err()
        }
        None => true,
    };

    if needs_restart {
        if let Some(mut existing) = slot.take() {
            existing.shutdown();
        }
        *slot = Some(PaddleWorkerClient::start(
            python_cmd,
            script_path,
            model_config,
        )?);
    }

    slot.as_mut()
        .ok_or_else(|| AppError::Unknown("PaddleOCR worker 初始化失败".to_string()))
}

fn parse_paddle_response_line(line: &str) -> Result<Option<OcrResult>> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
        AppError::Unknown(format!(
            "无法解析 PaddleOCR 输出: {e}; 行长度: {} 字节",
            line.len()
        ))
    })?;

    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        return Err(AppError::Unknown(format!("PaddleOCR 错误: {error}")));
    }

    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if text.trim().is_empty() {
        return Ok(None);
    }

    let confidence = value
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;

    let boxes: Vec<OcrBox> = value
        .get("boxes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    log::debug!(
        "PaddleOCR 识别到 {} 个字符, 置信度: {:.2}",
        text.len(),
        confidence
    );

    Ok(Some(OcrResult {
        text,
        confidence,
        boxes,
    }))
}

impl PaddleModelConfig {
    fn from_env() -> Self {
        Self {
            det_model_name: read_non_empty_env("WORK_REVIEW_PADDLE_DET_MODEL_NAME"),
            rec_model_name: read_non_empty_env("WORK_REVIEW_PADDLE_REC_MODEL_NAME"),
            ocr_version: read_non_empty_env("WORK_REVIEW_PADDLE_OCR_VERSION"),
        }
    }

    fn apply_to_command(&self, command: &mut Command) {
        if let Some(value) = self.det_model_name.as_deref() {
            command.env("WORK_REVIEW_PADDLE_DET_MODEL_NAME", value);
        }
        if let Some(value) = self.rec_model_name.as_deref() {
            command.env("WORK_REVIEW_PADDLE_REC_MODEL_NAME", value);
        }
        if let Some(value) = self.ocr_version.as_deref() {
            command.env("WORK_REVIEW_PADDLE_OCR_VERSION", value);
        }
    }
}

fn read_non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Copy)]
struct RetryRegionPlan {
    name: &'static str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn execute_ocr_pipeline_with<F>(image_path: &Path, mut recognize: F) -> Result<Option<OcrResult>>
where
    F: FnMut(&Path) -> Result<Option<OcrResult>>,
{
    let initial = recognize(image_path)?;
    if !should_retry_after_initial_ocr(initial.as_ref()) {
        return Ok(initial);
    }

    let source_image = match image::open(image_path) {
        Ok(image) => image,
        Err(error) => {
            log::debug!("加载 OCR 原图失败，保留首轮结果: {error}");
            return Ok(initial);
        }
    };

    let mut retry_results = Vec::new();
    for plan in build_retry_region_plans(source_image.width(), source_image.height()) {
        let cropped = crop_retry_region(&source_image, plan);
        let processed = preprocess_image_for_retry(&cropped);
        let temp_path = match save_retry_region_temp_image(&processed, plan.name) {
            Ok(path) => path,
            Err(error) => {
                log::debug!("保存 OCR 补救区域临时图失败: {error}");
                continue;
            }
        };

        let retry_result = recognize(&temp_path);
        let _ = std::fs::remove_file(&temp_path);

        match retry_result {
            Ok(Some(result)) if !result.text.trim().is_empty() => retry_results.push(result),
            Ok(_) => {}
            Err(error) => {
                log::debug!("OCR 补救区域 {} 识别失败: {error}", plan.name);
            }
        }
    }

    Ok(merge_ocr_results(initial, retry_results))
}

fn should_retry_after_initial_ocr(result: Option<&OcrResult>) -> bool {
    let Some(result) = result else {
        return true;
    };

    let raw = result.text.trim();
    if raw.is_empty() {
        return true;
    }

    let cleaned = clean_ocr_text(raw);
    if cleaned.is_empty() {
        return true;
    }

    let cleaned_len = cleaned.chars().count();
    let raw_len = raw.chars().count().max(1);
    let non_empty_lines = cleaned
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let useful_chars = cleaned
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    let useful_ratio = useful_chars as f32 / cleaned_len.max(1) as f32;
    let cleaned_ratio = cleaned_len as f32 / raw_len as f32;

    cleaned_len < 8
        || (cleaned_len < 24 && non_empty_lines < 2)
        || cleaned_ratio < 0.45
        || useful_ratio < 0.55
}

fn build_retry_region_plans(width: u32, height: u32) -> Vec<RetryRegionPlan> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    vec![
        clamp_retry_region(
            "top_strip",
            0,
            0,
            width,
            (height as f32 * 0.22) as u32,
            width,
            height,
        ),
        clamp_retry_region(
            "center_body",
            (width as f32 * 0.10) as u32,
            (height as f32 * 0.18) as u32,
            (width as f32 * 0.80) as u32,
            (height as f32 * 0.56) as u32,
            width,
            height,
        ),
        clamp_retry_region(
            "left_focus",
            (width as f32 * 0.04) as u32,
            (height as f32 * 0.16) as u32,
            (width as f32 * 0.62) as u32,
            (height as f32 * 0.60) as u32,
            width,
            height,
        ),
    ]
}

fn clamp_retry_region(
    name: &'static str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
) -> RetryRegionPlan {
    let x = x.min(image_width.saturating_sub(1));
    let y = y.min(image_height.saturating_sub(1));
    let width = width.max(1).min(image_width.saturating_sub(x).max(1));
    let height = height.max(1).min(image_height.saturating_sub(y).max(1));

    RetryRegionPlan {
        name,
        x,
        y,
        width,
        height,
    }
}

fn crop_retry_region(image: &DynamicImage, plan: RetryRegionPlan) -> DynamicImage {
    image.crop_imm(plan.x, plan.y, plan.width, plan.height)
}

fn preprocess_image_for_retry(image: &DynamicImage) -> DynamicImage {
    let expanded_width = ((image.width() as f32 * 1.6).round() as u32)
        .max(image.width().saturating_add(1))
        .min(2200);
    let expanded_height = (((image.height() as u64) * (expanded_width as u64))
        / image.width().max(1) as u64)
        .max(image.height().saturating_add(1) as u64)
        .min(2200) as u32;

    image
        .grayscale()
        .adjust_contrast(28.0)
        .resize(expanded_width, expanded_height, FilterType::Lanczos3)
        .unsharpen(0.8, 1)
}

fn save_retry_region_temp_image(image: &DynamicImage, label: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("work_review_ocr_retry");
    std::fs::create_dir_all(&dir)?;

    let label = sanitize_retry_region_label(label);
    let path = dir.join(format!("{}_{}.png", label, Uuid::new_v4()));
    image.save_with_format(&path, ImageFormat::Png)?;
    Ok(path)
}

fn sanitize_retry_region_label(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "retry".to_string()
    } else {
        sanitized
    }
}

fn merge_ocr_results(
    initial: Option<OcrResult>,
    retry_results: Vec<OcrResult>,
) -> Option<OcrResult> {
    let mut merged_lines = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(result) = initial.as_ref() {
        append_unique_clean_lines(&result.text, &mut merged_lines, &mut seen);
    }
    for result in &retry_results {
        append_unique_clean_lines(&result.text, &mut merged_lines, &mut seen);
    }

    if merged_lines.is_empty() {
        return None;
    }

    let boxes = initial
        .as_ref()
        .map(|result| result.boxes.clone())
        .unwrap_or_default();
    let confidence = initial
        .iter()
        .chain(retry_results.iter())
        .map(|result| result.confidence)
        .fold(0.0_f32, f32::max);

    Some(OcrResult {
        text: merged_lines.join("\n"),
        confidence,
        boxes,
    })
}

fn append_unique_clean_lines(
    text: &str,
    merged_lines: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    for line in clean_ocr_text(text).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_lowercase();
        if seen.insert(normalized) {
            merged_lines.push(trimmed.to_string());
        }
    }
}

/// 清理 OCR 文本（剔除乱码、Markdown 符号等）
pub fn clean_ocr_text(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in text.lines() {
        let cleaned = clean_line(line);

        // 跳过空行或太短的行（可能是乱码）。按字符数判断，避免单字中文（3 字节）漏过。
        if cleaned.chars().count() < 2 {
            continue;
        }

        // 跳过重复行
        let normalized = cleaned.to_lowercase();
        if seen.contains(&normalized) {
            continue;
        }
        seen.insert(normalized);

        lines.push(cleaned);
    }

    lines.join("\n")
}

/// 清理单行文本
fn clean_line(line: &str) -> String {
    let mut result = String::new();

    for c in line.chars() {
        // 只保留：中文、英文、数字、常用标点
        if is_valid_char(c) {
            result.push(c);
        } else if c.is_whitespace() {
            // 空格保留，但连续空格合并
            if !result.ends_with(' ') && !result.is_empty() {
                result.push(' ');
            }
        }
    }

    result.trim().to_string()
}

/// 判断字符是否有效（中英文、数字、常用标点、代码/URL 符号、日韩文）
fn is_valid_char(c: char) -> bool {
    // CJK 中文（基本区 + 扩展 A）
    if ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c) {
        return true;
    }
    // 日文假名（平假名 / 片假名）
    if ('\u{3040}'..='\u{30ff}').contains(&c) {
        return true;
    }
    // 韩文音节
    if ('\u{ac00}'..='\u{d7af}').contains(&c) {
        return true;
    }
    // 英文字母、数字
    if c.is_ascii_alphanumeric() {
        return true;
    }
    // 代码 / URL / 文件路径常用符号：OCR 屏幕上的代码、终端、URL 不应被这些符号破坏
    if matches!(
        c,
        '/' | '\\'
            | '*'
            | '+'
            | '='
            | '<'
            | '>'
            | '#'
            | '@'
            | '$'
            | '%'
            | '^'
            | '&'
            | '|'
            | '`'
            | '~'
            | '{'
            | '}'
            | '['
            | ']'
            | '_'
            | '-'
    ) {
        return true;
    }
    // 常用标点（中英文）
    let punctuation: [char; 30] = [
        '，', '。', '！', '？', '、', '；', '：', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}',
        '（', '）', '【', '】', '「', '」', '《', '》', '-', '—', '·', '.', ',', ':', ';', '!',
        '?', '(', ')',
    ];
    punctuation.contains(&c)
}

/// 常见密钥 / 令牌格式（OCR 常见于终端、IDE、控制台页面）。先于数字类规则执行，
/// 避免令牌中的数字段被手机号/卡号规则拆碎后残留其余部分。
static SECRET_TOKEN_PATTERNS: once_cell::sync::Lazy<Vec<regex::Regex>> = once_cell::sync::Lazy::new(
    || {
        [
            // 多行 PEM 私钥块（含正文与尾标，避免只删首行头而漏掉 Base64 私钥主体）
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
            // HTTP Bearer 令牌
            r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{16,}",
            // OpenAI / Anthropic 等 sk- 前缀密钥
            r"\bsk-[A-Za-z0-9_-]{16,}",
            // GitHub / GitLab 令牌
            r"\bgh[pousr]_[A-Za-z0-9]{20,}",
            r"\bgithub_pat_[A-Za-z0-9_]{20,}",
            r"\bglpat-[A-Za-z0-9_-]{16,}",
            // AWS Access Key ID
            r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
            // Slack 令牌
            r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
            r"\bxapp-\d-[A-Za-z0-9-]{16,}",
            // Google API / OAuth 令牌
            r"\bAIza[0-9A-Za-z_-]{35}\b",
            r"\bya29\.[0-9A-Za-z_-]{20,}",
            // JWT
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            // 常见环境变量形式（如 AWS_SECRET_ACCESS_KEY=... / GITHUB_TOKEN=...）
            r#"(?i)\b[A-Z0-9_]*(?:SECRET|API_KEY|ACCESS_KEY|AUTH_TOKEN|PRIVATE_KEY)[A-Z0-9_]*["']?\s*[:=：]\s*["']?[^\s"',;}\]]{8,}"#,
        ]
        .iter()
        .map(|pattern| regex::Regex::new(pattern).expect("内置密钥正则必须可编译"))
        .collect()
    },
);

static PHONE_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r"1[3-9]\d{9}").ok());
static ID_CARD_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| {
        regex::Regex::new(
            r"[1-9]\d{5}(?:18|19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]",
        )
        .ok()
    });
static BANK_CARD_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{0,3}").ok()
    });
static EMAIL_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").ok()
    });
static IPV4_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").ok()
    });
static PASSWORD_PATTERN: once_cell::sync::Lazy<Option<regex::Regex>> =
    once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r#"(?i)(?:password|密码|pwd|passwd)["']?[:：=\s]+["']?[^\s"',;}\]]+"#)
            .ok()
    });

fn replace_with_non_digit_boundary(text: &str, re: &regex::Regex, replacement: &str) -> String {
    let mut filtered = String::with_capacity(text.len());
    let mut last_end = 0;
    for m in re.find_iter(text) {
        let prev_is_digit = text[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
        let next_is_digit = text[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if prev_is_digit || next_is_digit {
            continue;
        }
        filtered.push_str(&text[last_end..m.start()]);
        filtered.push_str(replacement);
        last_end = m.end();
    }
    filtered.push_str(&text[last_end..]);
    filtered
}

/// 过滤敏感信息
pub fn filter_sensitive_text(text: &str) -> String {
    // 先清理乱码和特殊字符
    let cleaned = clean_ocr_text(text);
    let mut result = cleaned;

    // 密钥 / 令牌（含多行 PEM 私钥）
    for re in SECRET_TOKEN_PATTERNS.iter() {
        result = re.replace_all(&result, "[密钥]").to_string();
    }

    // 过滤身份证号（18 位，前后不得紧邻数字）——必须先于银行卡（避免末尾带 X 的 17 位被截成银行卡+X）与手机号执行
    if let Some(re) = ID_CARD_PATTERN.as_ref() {
        result = replace_with_non_digit_boundary(&result, re, "[身份证号]");
    }

    // 过滤银行卡号（16-19 位数字）——必须先于手机号执行，避免被 11 位手机号规则从中间截断
    if let Some(re) = BANK_CARD_PATTERN.as_ref() {
        result = replace_with_non_digit_boundary(&result, re, "[银行卡号]");
    }

    // 过滤手机号（11 位独立数字，不误伤 13 位时间戳或长订单号）
    if let Some(re) = PHONE_PATTERN.as_ref() {
        result = replace_with_non_digit_boundary(&result, re, "[手机号]");
    }

    // 过滤邮箱
    if let Some(re) = EMAIL_PATTERN.as_ref() {
        result = re.replace_all(&result, "[邮箱]").to_string();
    }

    // 过滤 IP 地址
    if let Some(re) = IPV4_PATTERN.as_ref() {
        result = re.replace_all(&result, "[IP地址]").to_string();
    }

    // 过滤密码相关
    if let Some(re) = PASSWORD_PATTERN.as_ref() {
        result = re.replace_all(&result, "[密码信息]").to_string();
    }

    result
}

/// OCR 文本入库前的完整脱敏：内置敏感模式（含密钥/令牌格式）+ 隐私过滤器的通用规则
/// （`api_key|secret|token: 值` 等，与窗口标题同一套）。
///
/// 不按"敏感关键词"整段丢弃：默认关键词含 login / password 等常见词，整屏 OCR 几乎必然命中；
/// 关键词仍只作用于窗口标题（命中即脱敏且不截图、不 OCR）。
pub fn redact_ocr_for_storage(
    text: &str,
    privacy_filter: &work_review_core::privacy::PrivacyFilter,
) -> String {
    privacy_filter.filter_text(&filter_sensitive_text(text))
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_ocr应在常驻进程内按引擎上限缩放后识别() {
        // 非 Windows CI 也能检查内嵌脚本，避免 cfg(windows) 隐藏此回归。
        let source = include_str!("ocr.rs");
        let entry = source
            .split("pub fn extract_text(&self")
            .nth(1)
            .unwrap()
            .split("fn extract_text_once(")
            .next()
            .unwrap();
        assert!(entry.contains("if self.preferred_engine == OcrEngine::WindowsOCR"));
        assert!(entry.contains("return self.extract_text_once(image_path);"));

        let worker_block = source
            .split("const WINDOWS_OCR_WORKER_SCRIPT: &str = r#\"")
            .nth(1)
            .unwrap()
            .split("struct PaddleWorkerClient")
            .next()
            .unwrap();
        let limit = worker_block
            .find("$maxDimension = [Windows.Media.Ocr.OcrEngine]::MaxImageDimension")
            .unwrap();
        let decode = worker_block
            .find("$bitmap = Await ($decoder.GetSoftwareBitmapAsync(")
            .unwrap();
        let recognize = worker_block
            .find("$ocrEngine.RecognizeAsync($ocrBitmap)")
            .unwrap();
        assert!(limit < decode && decode < recognize);
        assert!(worker_block
            .contains("if ([Math]::Max($sourceWidth, $sourceHeight) -gt $maxDimension)"));
        assert!(worker_block
            .contains("$scale = [double]$maxDimension / [Math]::Max($sourceWidth, $sourceHeight)"));
        for dimension in ["Width", "Height"] {
            assert!(worker_block.contains(&format!(
                "$transform.Scaled{dimension} = [uint32][Math]::Max(1, [Math]::Floor($source{dimension} * $scale))"
            )));
        }
        assert!(worker_block.contains(
            "$transform,\n                [Windows.Graphics.Imaging.ExifOrientationMode]::IgnoreExifOrientation"
        ));
        assert!(worker_block
            .contains("} else {\n            $bitmap = Await ($decoder.GetSoftwareBitmapAsync())"));
        assert!(worker_block.contains("New-Object System.Collections.Generic.List[string]"));
        assert!(
            !worker_block.contains("$boxes +="),
            "不应保留 O(n^2) 数组拼接"
        );
        assert_eq!(worker_block.matches("RecognizeAsync(").count(), 1);
        assert_eq!(worker_block.matches("Command::new(").count(), 1);
        assert!(worker_block.contains("\"-WindowStyle\",\n                \"Hidden\""));
        assert!(worker_block.contains(".creation_flags(CREATE_NO_WINDOW)"));
    }

    use super::{
        build_retry_region_plans, execute_ocr_pipeline_with, merge_ocr_results,
        preprocess_image_for_retry, should_retry_after_initial_ocr, OcrResult,
    };
    #[cfg(unix)]
    use super::{paddle_worker_slot, request_paddle_via_worker, PaddleModelConfig};
    use image::{DynamicImage, Rgba, RgbaImage};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::sync::{Mutex, OnceLock};
    use uuid::Uuid;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn sample_result(text: &str) -> OcrResult {
        OcrResult {
            text: text.to_string(),
            confidence: 0.9,
            boxes: vec![],
        }
    }

    #[test]
    fn 内置敏感信息正则必须全部可编译() {
        // 任何一条编译失败都会被 if let 静默跳过，导致该类信息从未被过滤（银行卡规则曾因此失效）
        assert_eq!(super::SECRET_TOKEN_PATTERNS.len(), 13);
        assert!(super::PHONE_PATTERN.is_some());
        assert!(super::ID_CARD_PATTERN.is_some());
        assert!(super::BANK_CARD_PATTERN.is_some());
        assert!(super::EMAIL_PATTERN.is_some());
        assert!(super::IPV4_PATTERN.is_some());
        assert!(super::PASSWORD_PATTERN.is_some());
    }

    #[test]
    fn 常见密钥与令牌格式应被过滤() {
        let samples = [
            "OPENAI_KEY sk-proj-abcdefghijklmnopqrstuvwx1234",
            "Authorization: Bearer abcdefghijklmnop1234567890",
            "token ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "gitlab glpat-abcdefghij1234567890",
            "google ya29.a0AfH6SMCdefghijklmnop1234567890",
            "slack-app xapp-1-A0123456789-abcdef123456",
            "AKIAABCDEFGHIJKLMNOP is the access key",
            "slack xoxb-123456789012-abcdefghij",
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAtestbody\n-----END RSA PRIVATE KEY-----",
        ];
        for sample in samples {
            let masked = super::filter_sensitive_text(sample);
            assert!(masked.contains("[密钥]"), "未过滤: {sample} -> {masked}");
            assert!(
                !masked.contains("MIIEowIBAAKCAQEAtestbody"),
                "多行 PEM 私钥正文未脱敏: {masked}"
            );
        }
        // 18 位身份证号（前段含 1[3-9] 且末位为 X）必须优先按身份证脱敏，不得被银行卡或手机号截断
        let id_masked = super::filter_sensitive_text("证件号：13800119900101123X");
        assert_eq!(id_masked, "证件号：[身份证号]");
        // 普通代码文本不应被误伤
        let code = super::filter_sensitive_text("fn login_user(name: &str) -> bool");
        assert_eq!(code, "fn login_user(name: &str) -> bool");
    }

    #[test]
    fn ocr入库脱敏应同时应用窗口标题的通用规则() {
        let filter = work_review_core::privacy::PrivacyFilter::from_config(
            &work_review_core::config::PrivacyConfig::default(),
        );
        let masked =
            super::redact_ocr_for_storage("api_key = my-internal-value 其余正常文本", &filter);
        assert!(!masked.contains("my-internal-value"), "{masked}");
        assert!(masked.contains("其余正常文本"));
        // 默认敏感关键词（login 等）不应导致整段 OCR 被丢弃
        let kept = super::redact_ocr_for_storage("Login 页面 欢迎回来", &filter);
        assert!(kept.contains("欢迎回来"));
    }

    #[test]
    fn 银行卡号应被过滤且不误伤更长数字串() {
        // 连续 16 位与 4-4-4-4 分隔两种形态都应被打码
        let masked =
            super::filter_sensitive_text("卡号 6222020200112233 或 6222-0202-0011-2233 备用");
        assert!(!masked.contains("6222020200112233"), "实际输出: {masked}");
        assert!(!masked.contains("2233"), "实际输出: {masked}");
        assert!(masked.contains("[银行卡号]"), "实际输出: {masked}");

        // 匹配段之后紧跟数字（整体超出 16-19 位,如 4 位一组的 20 位编号）时不应误报
        let longer = super::filter_sensitive_text("编号 6222-0202-0011-2233-4444 结束");
        assert!(!longer.contains("[银行卡号]"), "实际输出: {longer}");
        assert!(
            longer.contains("6222-0202-0011-2233-4444"),
            "实际输出: {longer}"
        );
    }

    fn create_test_image(width: u32, height: u32) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("work_review_ocr_test_{}.png", Uuid::new_v4()));
        let image = RgbaImage::from_pixel(width, height, Rgba([220, 230, 240, 255]));
        DynamicImage::ImageRgba8(image).save(&path).unwrap();
        path
    }

    fn create_test_data_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("work_review_ocr_data_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn paddle_worker_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[cfg(unix)]
    fn reset_paddle_worker_for_test() {
        let mut slot = paddle_worker_slot().lock().unwrap();
        if let Some(mut worker) = slot.take() {
            worker.shutdown();
        }
    }

    #[cfg(unix)]
    fn create_fake_paddle_python(crash_on_first_request: bool) -> (PathBuf, PathBuf, PathBuf) {
        let state_dir =
            std::env::temp_dir().join(format!("work_review_fake_paddle_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&state_dir).unwrap();

        if crash_on_first_request {
            std::fs::write(state_dir.join("crash_once"), b"1").unwrap();
        }

        let python_path = state_dir.join("fake_python.sh");
        let wrapper = r#"#!/bin/sh
set -eu

state_dir="${WORK_REVIEW_FAKE_PADDLE_STATE_DIR:?}"
start_file="$state_dir/start_count"
request_file="$state_dir/request_count"
crash_once_file="$state_dir/crash_once"
crash_marker="$state_dir/crash_marker"
det_model_file="$state_dir/det_model_name"
rec_model_file="$state_dir/rec_model_name"
ocr_version_file="$state_dir/ocr_version"

read_counter() {
    if [ -f "$1" ]; then
        cat "$1"
    else
        echo 0
    fi
}

increment_counter() {
    current=$(read_counter "$1")
    next=$((current + 1))
    echo "$next" > "$1"
    echo "$next"
}

start_count=$(increment_counter "$start_file")

printf '%s' "${WORK_REVIEW_PADDLE_DET_MODEL_NAME:-}" > "$det_model_file"
printf '%s' "${WORK_REVIEW_PADDLE_REC_MODEL_NAME:-}" > "$rec_model_file"
printf '%s' "${WORK_REVIEW_PADDLE_OCR_VERSION:-}" > "$ocr_version_file"

if [ "${3:-}" = "--worker" ]; then
    printf '%s\n' '{"status":"ready"}'
    while IFS= read -r line; do
        case "$line" in
            *'"command":"shutdown"'*)
                exit 0
                ;;
        esac

        request_count=$(increment_counter "$request_file")

        if [ -f "$crash_once_file" ] && [ ! -f "$crash_marker" ]; then
            : > "$crash_marker"
            exit 1
        fi

        text="mock-${start_count}-${request_count}"
        printf '{"text":"%s","boxes":[],"confidence":0.9}\n' "$text"
    done
    exit 0
fi

printf '%s\n' '{"text":"single","boxes":[],"confidence":0.9}'
"#;

        std::fs::write(&python_path, wrapper).unwrap();
        let mut permissions = std::fs::metadata(&python_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&python_path, permissions).unwrap();

        let script_path = state_dir.join("fake_script.py");
        std::fs::write(&script_path, "# fake paddle script\n").unwrap();

        (python_path, script_path, state_dir)
    }

    #[cfg(unix)]
    fn read_counter(path: &Path) -> u32 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }

    #[test]
    fn 低质量判定应在短文本和空文本时触发补救() {
        assert!(should_retry_after_initial_ocr(None));
        assert!(should_retry_after_initial_ocr(Some(&sample_result("abc"))));
        assert!(should_retry_after_initial_ocr(Some(&sample_result(
            "  - -  "
        ))));
        assert!(!should_retry_after_initial_ocr(Some(&sample_result(
            "项目进展\n完成接口联调\n准备提交测试"
        ))));
    }

    #[test]
    fn 区域规划应生成不越界的高价值区域() {
        let plans = build_retry_region_plans(1440, 900);
        assert!(!plans.is_empty());
        assert!(plans.iter().all(|plan| plan.width > 0 && plan.height > 0));
        assert!(plans.iter().all(|plan| plan.x + plan.width <= 1440));
        assert!(plans.iter().all(|plan| plan.y + plan.height <= 900));
    }

    #[test]
    fn 预处理应放大区域图并保留非零尺寸() {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(240, 80, Rgba([80, 90, 100, 255])));
        let processed = preprocess_image_for_retry(&image);
        assert!(processed.width() > image.width());
        assert!(processed.height() > image.height());
    }

    #[test]
    fn 合并结果应去重并补充新增文本() {
        let merged = merge_ocr_results(
            Some(sample_result("项目看板\nIssue 28")),
            vec![
                sample_result("Issue 28\nPull Request"),
                sample_result("项目看板\n接口联调"),
            ],
        )
        .unwrap();

        assert_eq!(merged.text, "项目看板\nIssue 28\nPull Request\n接口联调");
    }

    #[test]
    fn 高质量首轮结果不应触发补救ocr() {
        let image_path = create_test_image(1280, 800);
        let mut calls = 0;

        let result = execute_ocr_pipeline_with(&image_path, |_: &Path| {
            calls += 1;
            Ok(Some(sample_result("项目进展\n完成接口联调\n准备提交测试")))
        })
        .unwrap()
        .unwrap();

        let _ = std::fs::remove_file(&image_path);

        assert_eq!(calls, 1);
        assert!(result.text.contains("完成接口联调"));
    }

    #[test]
    fn 低质量首轮结果应触发区域补救并合并文本() {
        let image_path = create_test_image(1280, 800);
        let mut responses = VecDeque::from([
            Some(sample_result("abc")),
            Some(sample_result("顶部标题")),
            Some(sample_result("正文内容")),
            None,
        ]);
        let mut calls = 0;

        let result = execute_ocr_pipeline_with(&image_path, |_| {
            calls += 1;
            Ok(responses.pop_front().unwrap_or(None))
        })
        .unwrap()
        .unwrap();

        let _ = std::fs::remove_file(&image_path);

        assert!(calls > 1);
        assert!(result.text.contains("顶部标题"));
        assert!(result.text.contains("正文内容"));
    }

    #[test]
    fn 已有旧版paddle脚本应在服务初始化时升级为worker版本() {
        let data_dir = create_test_data_dir();
        let script_path = data_dir.join("paddle_ocr.py");
        std::fs::write(&script_path, "#!/usr/bin/env python3\nprint('legacy')\n").unwrap();

        let _service = super::OcrService::new(&data_dir);
        let updated = std::fs::read_to_string(&script_path).unwrap();

        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(updated.contains("def run_worker():"));
        assert!(updated.contains("if len(sys.argv) >= 2 and sys.argv[1] == \"--worker\":"));
    }

    #[test]
    fn 生成的paddle脚本应兼容字典式ocr结果结构() {
        let data_dir = create_test_data_dir();
        let script_path = data_dir.join("paddle_ocr.py");

        let _service = super::OcrService::new(&data_dir);
        let script = std::fs::read_to_string(&script_path).unwrap();

        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(script.contains("res.get('rec_texts', [])"));
        assert!(script.contains("res.get('rec_scores', [])"));
        assert!(script.contains("res.get('dt_polys', [])"));
    }

    #[test]
    fn 生成的paddle脚本应支持从环境变量读取模型配置() {
        let data_dir = create_test_data_dir();
        let script_path = data_dir.join("paddle_ocr.py");

        let _service = super::OcrService::new(&data_dir);
        let script = std::fs::read_to_string(&script_path).unwrap();

        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(script.contains("WORK_REVIEW_PADDLE_DET_MODEL_NAME"));
        assert!(script.contains("WORK_REVIEW_PADDLE_REC_MODEL_NAME"));
        assert!(script.contains("WORK_REVIEW_PADDLE_OCR_VERSION"));
    }

    #[cfg(unix)]
    #[test]
    fn paddle_worker应在多次请求间复用单个进程() {
        let _guard = paddle_worker_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_paddle_worker_for_test();

        let (python_path, script_path, state_dir) = create_fake_paddle_python(false);
        let image_path = create_test_image(640, 480);
        std::env::set_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR", &state_dir);

        let first = request_paddle_via_worker(
            &python_path.to_string_lossy(),
            &script_path,
            &image_path,
            &PaddleModelConfig::default(),
        )
        .unwrap()
        .unwrap();
        let second = request_paddle_via_worker(
            &python_path.to_string_lossy(),
            &script_path,
            &image_path,
            &PaddleModelConfig::default(),
        )
        .unwrap()
        .unwrap();
        let start_count = read_counter(&state_dir.join("start_count"));

        reset_paddle_worker_for_test();
        std::env::remove_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR");
        let _ = std::fs::remove_file(&image_path);
        let _ = std::fs::remove_dir_all(&state_dir);

        assert_eq!(first.text, "mock-1-1");
        assert_eq!(second.text, "mock-1-2");
        assert_eq!(start_count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn paddle_worker崩溃后应自动重启一次并完成重试() {
        let _guard = paddle_worker_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_paddle_worker_for_test();

        let (python_path, script_path, state_dir) = create_fake_paddle_python(true);
        let image_path = create_test_image(640, 480);
        std::env::set_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR", &state_dir);

        let recovered = request_paddle_via_worker(
            &python_path.to_string_lossy(),
            &script_path,
            &image_path,
            &PaddleModelConfig::default(),
        )
        .unwrap()
        .unwrap();
        let start_count = read_counter(&state_dir.join("start_count"));

        reset_paddle_worker_for_test();
        std::env::remove_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR");
        let _ = std::fs::remove_file(&image_path);
        let _ = std::fs::remove_dir_all(&state_dir);

        assert!(recovered.text.starts_with("mock-2-"));
        assert_eq!(start_count, 2);
    }

    #[cfg(unix)]
    #[test]
    fn paddle_worker在模型配置变化后应重建并透传环境变量() {
        let _guard = paddle_worker_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_paddle_worker_for_test();

        let (python_path, script_path, state_dir) = create_fake_paddle_python(false);
        let image_path = create_test_image(640, 480);
        std::env::set_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR", &state_dir);

        let first_config = PaddleModelConfig {
            det_model_name: Some("PP-OCRv5_server_det".to_string()),
            rec_model_name: Some("PP-OCRv5_server_rec".to_string()),
            ocr_version: Some("PP-OCRv5".to_string()),
        };
        let second_config = PaddleModelConfig {
            det_model_name: Some("PP-OCRv5_mobile_det".to_string()),
            rec_model_name: Some("PP-OCRv5_mobile_rec".to_string()),
            ocr_version: Some("PP-OCRv5".to_string()),
        };

        let _ = request_paddle_via_worker(
            &python_path.to_string_lossy(),
            &script_path,
            &image_path,
            &first_config,
        )
        .unwrap();
        let first_start_count = read_counter(&state_dir.join("start_count"));
        let first_det_model = std::fs::read_to_string(state_dir.join("det_model_name")).unwrap();
        let first_rec_model = std::fs::read_to_string(state_dir.join("rec_model_name")).unwrap();

        let _ = request_paddle_via_worker(
            &python_path.to_string_lossy(),
            &script_path,
            &image_path,
            &second_config,
        )
        .unwrap();
        let second_start_count = read_counter(&state_dir.join("start_count"));
        let second_det_model = std::fs::read_to_string(state_dir.join("det_model_name")).unwrap();
        let second_rec_model = std::fs::read_to_string(state_dir.join("rec_model_name")).unwrap();

        reset_paddle_worker_for_test();
        std::env::remove_var("WORK_REVIEW_FAKE_PADDLE_STATE_DIR");
        let _ = std::fs::remove_file(&image_path);
        let _ = std::fs::remove_dir_all(&state_dir);

        assert_eq!(first_start_count, 1);
        assert_eq!(first_det_model, "PP-OCRv5_server_det");
        assert_eq!(first_rec_model, "PP-OCRv5_server_rec");
        assert_eq!(second_start_count, 2);
        assert_eq!(second_det_model, "PP-OCRv5_mobile_det");
        assert_eq!(second_rec_model, "PP-OCRv5_mobile_rec");
    }
}
