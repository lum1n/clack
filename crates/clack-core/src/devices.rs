use crate::error::{mic_unavailable, ClackResult};

/// One capture device the UI can offer. `id` is what you pass back to the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[cfg(all(feature = "microphone", not(target_arch = "wasm32")))]
pub fn input_devices() -> ClackResult<Vec<InputDevice>> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let devices = host.input_devices().map_err(mic_unavailable)?;
    let mut listed = Vec::new();
    for (index, device) in devices.enumerate() {
        let (id, name) = device_label(&device, index);
        let is_default = default_id.as_deref() == Some(id.as_str());
        listed.push(InputDevice { id, name, is_default });
    }
    if listed.is_empty() {
        return Err(mic_unavailable("no input devices"));
    }
    if !listed.iter().any(|device| device.is_default) {
        listed[0].is_default = true;
    }
    Ok(listed)
}

#[cfg(not(all(feature = "microphone", not(target_arch = "wasm32"))))]
pub fn input_devices() -> ClackResult<Vec<InputDevice>> {
    Err(mic_unavailable(
        "this build has no microphone capture, so it cannot list input devices",
    ))
}

#[cfg(all(feature = "microphone", not(target_arch = "wasm32")))]
pub(crate) fn open_input_stream(
    device_id: Option<&str>,
    inbox: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
) -> ClackResult<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = match device_id {
        Some(id) => {
            let parsed = id.parse::<cpal::DeviceId>().map_err(|_| {
                mic_unavailable(format!("no input device with id {id}"))
            })?;
            host.device_by_id(&parsed)
                .ok_or_else(|| mic_unavailable(format!("no input device with id {id}")))?
        }
        None => host
            .default_input_device()
            .or_else(|| host.input_devices().ok().and_then(|mut devices| devices.next()))
            .ok_or_else(|| mic_unavailable("no input devices"))?,
    };
    let supported = device.default_input_config().map_err(mic_unavailable)?;
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config = supported.config();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let inbox = inbox.clone();
            device.build_input_stream(
                config,
                move |data: &[f32], _| push_frames(data, channels, sample_rate, &inbox),
                log_stream_err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let inbox = inbox.clone();
            device.build_input_stream(
                config,
                move |data: &[i16], _| {
                    let floats: Vec<f32> =
                        data.iter().map(|sample| *sample as f32 / 32768.0).collect();
                    push_frames(&floats, channels, sample_rate, &inbox);
                },
                log_stream_err,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let inbox = inbox.clone();
            device.build_input_stream(
                config,
                move |data: &[u16], _| {
                    let floats: Vec<f32> = data
                        .iter()
                        .map(|sample| (*sample as f32 - 32768.0) / 32768.0)
                        .collect();
                    push_frames(&floats, channels, sample_rate, &inbox);
                },
                log_stream_err,
                None,
            )
        }
        other => {
            return Err(mic_unavailable(format!(
                "unsupported sample format {other:?}"
            )))
        }
    }
    .map_err(|err| mic_unavailable(err))?;
    stream.play().map_err(|err| mic_unavailable(err))?;
    Ok(stream)
}

#[cfg(all(feature = "microphone", not(target_arch = "wasm32")))]
fn device_label(device: &cpal::Device, index: usize) -> (String, String) {
    use cpal::traits::DeviceTrait;
    let name = device
        .description()
        .map(|description| description.name().to_string())
        .unwrap_or_else(|_| format!("Microphone {index}"));
    let id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| name.clone());
    (id, name)
}

#[cfg(all(feature = "microphone", not(target_arch = "wasm32")))]
fn log_stream_err(err: cpal::Error) {
    eprintln!("clack microphone: {err}");
}

#[cfg(all(feature = "microphone", not(target_arch = "wasm32")))]
fn push_frames(
    data: &[f32],
    channels: usize,
    sample_rate: u32,
    inbox: &std::sync::Mutex<Vec<f32>>,
) {
    let pcm = crate::audio::interleaved_to_16k(data, channels, sample_rate);
    if pcm.is_empty() {
        return;
    }
    inbox
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .extend(pcm);
}
