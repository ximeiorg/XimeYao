//! WASAPI 麦克风采集（共享模式）+ 混音格式 → 单声道 f32 转换。
//!
//! 采集按**会话**开关：`Capture::open()` 打开默认通信设备（eCommunications，
//! 即系统「麦克风」角色），`Drop` 时 `Stop()` 并释放——空闲时不占麦克风。
//!
//! 采样率按设备混音格式（常见 48kHz）原样交给识别器：sherpa-onnx 内部会
//! 重采样到模型要求的 16k（见 sherpa C API 头注释），这里不做重采样。
//! 多声道在这里混成单声道（ASR 只需一路；取平均比取首声道对相位差更稳）。

use std::ffi::c_void;

use windows::Win32::Media::Audio::{
    eCapture, eCommunications, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, CLSCTX_ALL,
};

/// WAVE_FORMAT_* 常量（不依赖 windows crate 的导出位置，避免版本间命名漂移）。
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// 共享模式缓冲区时长（100ns 单位）：200ms。识别器每 10ms 轮询取一次，
/// 这个缓冲足够吸收 UI 线程偶发卡顿而不丢样本。
const BUFFER_DURATION_HNS: i64 = 2_000_000;

/// 采集样本格式（只支持 ASR 需要的两种；24bit/8bit 直接报错，不做静默降级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// 32bit 浮点（共享模式混音格式的常态）。
    F32,
    /// 16bit 整数。
    I16,
}

impl SampleFormat {
    fn bytes_per_sample(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::I16 => 2,
        }
    }
}

/// 一次采集会话（Drop 即停止并释放设备）。
pub struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    format: SampleFormat,
    channels: u16,
    sample_rate: i32,
}

impl Capture {
    /// 打开默认通信采集设备（麦克风）并开始采集。
    ///
    /// 调用线程需已初始化 COM（server 的语音工作线程为 MTA）。
    pub fn open() -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("取得音频设备枚举器失败: {e}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eCapture, eCommunications)
                .map_err(|e| format!("找不到默认麦克风（eCommunications）: {e}"))?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("激活音频客户端失败: {e}"))?;

            let mix_ptr = client
                .GetMixFormat()
                .map_err(|e| format!("读取混音格式失败: {e}"))?;
            // 解析 + Initialize 都要用这块 COM 内存，用完（无论成败）再归还。
            let parsed = parse_mix_format(mix_ptr);
            let inited = match &parsed {
                Ok(_) => client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        0,
                        BUFFER_DURATION_HNS,
                        0,
                        mix_ptr,
                        None,
                    )
                    .map_err(|e| format!("初始化采集流失败: {e}")),
                Err(e) => Err(e.clone()),
            };
            CoTaskMemFree(Some(mix_ptr as *const c_void));
            let (format, channels, sample_rate) = parsed?;
            inited?;

            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|e| format!("取得采集客户端失败: {e}"))?;
            client.Start().map_err(|e| format!("启动采集失败: {e}"))?;

            Ok(Self {
                client,
                capture,
                format,
                channels,
                sample_rate,
            })
        }
    }

    /// 设备混音采样率（交给识别器做内部重采样）。
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// 取走当前可读的全部样本（单声道 f32）；无数据时返回空 Vec。
    pub fn read_chunk(&self) -> Result<Vec<f32>, String> {
        let mut out = Vec::new();
        loop {
            let packet = unsafe {
                self.capture
                    .GetNextPacketSize()
                    .map_err(|e| format!("查询采集包失败: {e}"))?
            };
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| format!("读取采集缓冲失败: {e}"))?
            };
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            if frames > 0 {
                if silent || data.is_null() {
                    // 静音包：喂等长零样本，保持时间轴连续（端点检测靠它判停顿）。
                    out.extend(std::iter::repeat(0.0f32).take(frames as usize));
                } else {
                    let bytes = frames as usize
                        * self.channels as usize
                        * self.format.bytes_per_sample();
                    let slice = unsafe { std::slice::from_raw_parts(data, bytes) };
                    out.extend(mix_down_to_mono_f32(slice, self.format, self.channels));
                }
            }
            unsafe {
                self.capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| format!("归还采集缓冲失败: {e}"))?
            };
        }
        Ok(out)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// 解析混音格式：返回（样本格式，声道数，采样率）。
///
/// 只接受 32bit 浮点与 16bit 整数（共享模式混音格式的实际取值）；
/// 其余位数明确报错，避免按错误步长解读缓冲。
unsafe fn parse_mix_format(ptr: *const WAVEFORMATEX) -> Result<(SampleFormat, u16, i32), String> {
    if ptr.is_null() {
        return Err("混音格式指针为空".to_string());
    }
    let fmt = unsafe { &*ptr };
    let format = match (fmt.wFormatTag, fmt.wBitsPerSample) {
        (WAVE_FORMAT_IEEE_FLOAT, _) => SampleFormat::F32,
        (WAVE_FORMAT_EXTENSIBLE, 32) => SampleFormat::F32,
        (WAVE_FORMAT_PCM, 16) | (WAVE_FORMAT_EXTENSIBLE, 16) => SampleFormat::I16,
        (tag, bits) => {
            return Err(format!("不支持的采集格式：wFormatTag={tag} wBitsPerSample={bits}"));
        }
    };
    if fmt.nChannels == 0 {
        return Err("采集格式声道数为 0".to_string());
    }
    if fmt.nSamplesPerSec == 0 {
        return Err("采集格式采样率为 0".to_string());
    }
    Ok((format, fmt.nChannels, fmt.nSamplesPerSec as i32))
}

/// 交织多声道字节流 → 单声道 f32（各声道取平均）。
pub fn mix_down_to_mono_f32(bytes: &[u8], format: SampleFormat, channels: u16) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }
    let ch = channels as usize;
    let per_sample = format.bytes_per_sample();
    let frames = bytes.len() / (per_sample * ch);
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        let mut sum = 0.0f32;
        for channel in 0..ch {
            let index = frame * ch + channel;
            let value = match format {
                SampleFormat::F32 => {
                    let off = index * 4;
                    f32::from_le_bytes([
                        bytes[off],
                        bytes[off + 1],
                        bytes[off + 2],
                        bytes[off + 3],
                    ])
                }
                SampleFormat::I16 => {
                    let off = index * 2;
                    i16::from_le_bytes([bytes[off], bytes[off + 1]]) as f32 / 32768.0
                }
            };
            sum += value;
        }
        out.push(sum / ch as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn stereo_float_is_averaged_into_mono() {
        // 帧0 = (1.0, -1.0) → 0.0；帧1 = (0.5, 0.5) → 0.5
        let bytes = f32_bytes(&[1.0, -1.0, 0.5, 0.5]);
        let mono = mix_down_to_mono_f32(&bytes, SampleFormat::F32, 2);
        assert_eq!(mono.len(), 2);
        assert!(mono[0].abs() < f32::EPSILON);
        assert!((mono[1] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn mono_i16_is_scaled_to_unit_range() {
        let bytes: Vec<u8> = [0i16, 32767, -32768]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mono = mix_down_to_mono_f32(&bytes, SampleFormat::I16, 1);
        assert_eq!(mono.len(), 3);
        assert!(mono[0].abs() < f32::EPSILON);
        assert!(mono[1] > 0.99);
        assert!((mono[2] + 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn trailing_partial_frame_is_ignored_and_zero_channels_yields_nothing() {
        // 立体声 f32：6 字节不足一帧（8 字节）→ 0 帧
        let mono = mix_down_to_mono_f32(&[0u8; 6], SampleFormat::F32, 2);
        assert!(mono.is_empty());
        assert!(mix_down_to_mono_f32(&[0u8; 8], SampleFormat::F32, 0).is_empty());
    }

    #[test]
    fn mix_format_accepts_float32_and_rejects_odd_bit_depth() {
        let float32 = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 384_000,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let parsed = unsafe { parse_mix_format(&float32) };
        assert_eq!(parsed.map_err(|e| e.to_string()), Ok((SampleFormat::F32, 2, 48_000)));

        let pcm24 = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: 1,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 144_000,
            nBlockAlign: 3,
            wBitsPerSample: 24,
            cbSize: 0,
        };
        assert!(unsafe { parse_mix_format(&pcm24) }.is_err());

        // 空指针与非零声道兜底
        assert!(unsafe { parse_mix_format(std::ptr::null()) }.is_err());
        let zero_channels = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 0,
            nSamplesPerSec: 48_000,
            ..Default::default()
        };
        assert!(unsafe { parse_mix_format(&zero_channels) }.is_err());
    }
}