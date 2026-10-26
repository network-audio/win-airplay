use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

pub fn initialize() -> Result<(), String> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| error.to_string())
}

pub struct DefaultOutput {
    pub name: String,
    pub volume: crate::airplay::Volume,
    pub channel_gains: Vec<f32>,
}

pub fn default_render_output() -> Result<DefaultOutput, String> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| error.to_string())?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|error| error.to_string())?;
    let properties =
        unsafe { device.OpenPropertyStore(STGM_READ) }.map_err(|error| error.to_string())?;
    let name = unsafe { properties.GetValue(&PKEY_Device_FriendlyName) }
        .map_err(|error| error.to_string())?;
    let endpoint_volume: IAudioEndpointVolume =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let scalar = unsafe { endpoint_volume.GetMasterVolumeLevelScalar() }
        .map_err(|error| error.to_string())?;
    let muted = unsafe { endpoint_volume.GetMute() }
        .map_err(|error| error.to_string())?
        .as_bool();
    let count = unsafe { endpoint_volume.GetChannelCount() }.unwrap_or(0);
    let channel_gains = (0..count)
        .map(|channel| {
            unsafe { endpoint_volume.GetChannelVolumeLevelScalar(channel) }.unwrap_or(1.0)
        })
        .collect();
    Ok(DefaultOutput {
        name: name.to_string(),
        volume: crate::airplay::Volume { scalar, muted },
        channel_gains,
    })
}

pub fn set_default_volume_if_name(expected_name: &str, scalar: f32) -> Result<(), String> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| error.to_string())?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|error| error.to_string())?;
    let properties =
        unsafe { device.OpenPropertyStore(STGM_READ) }.map_err(|error| error.to_string())?;
    let name = unsafe { properties.GetValue(&PKEY_Device_FriendlyName) }
        .map_err(|error| error.to_string())?
        .to_string();
    if !name.contains(expected_name) {
        return Ok(());
    }
    let endpoint_volume: IAudioEndpointVolume =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let current = unsafe { endpoint_volume.GetMasterVolumeLevelScalar() }
        .map_err(|error| error.to_string())?;
    if (current - scalar).abs() >= 0.005 {
        unsafe { endpoint_volume.SetMasterVolumeLevelScalar(scalar, std::ptr::null()) }
            .map_err(|error| error.to_string())?;
    }
    let muted = unsafe { endpoint_volume.GetMute() }
        .map_err(|error| error.to_string())?
        .as_bool();
    let should_mute = scalar <= 0.0;
    if muted != should_mute {
        unsafe { endpoint_volume.SetMute(BOOL::from(should_mute), std::ptr::null()) }
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Render through a named endpoint without changing the system default output.
pub fn play_test_tone(expected_name: &str, duration: std::time::Duration) -> Result<(), String> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| error.to_string())?;
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|error| error.to_string())?;
    let count = unsafe { collection.GetCount() }.map_err(|error| error.to_string())?;
    let mut chosen = None;
    for index in 0..count {
        let device = unsafe { collection.Item(index) }.map_err(|error| error.to_string())?;
        let store =
            unsafe { device.OpenPropertyStore(STGM_READ) }.map_err(|error| error.to_string())?;
        let name = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }
            .map_err(|error| error.to_string())?
            .to_string();
        if name.contains(expected_name) {
            chosen = Some(device);
            break;
        }
    }
    let device = chosen
        .ok_or_else(|| format!("Audio endpoint containing {expected_name:?} was not found"))?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let format = WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 2,
        nSamplesPerSec: 44_100,
        nAvgBytesPerSec: 44_100 * 4,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 500_000, 0, &format, None) }
        .map_err(|error| format!("WASAPI render initialization failed: {error}"))?;
    let render: IAudioRenderClient =
        unsafe { client.GetService() }.map_err(|error| format!("GetService: {error}"))?;
    let buffer_frames =
        unsafe { client.GetBufferSize() }.map_err(|error| format!("GetBufferSize: {error}"))?;
    unsafe { client.Start() }.map_err(|error| format!("Start: {error}"))?;
    let deadline = std::time::Instant::now() + duration;
    let mut frame = 0_u64;
    while std::time::Instant::now() < deadline {
        let padding = unsafe { client.GetCurrentPadding() }
            .map_err(|error| format!("GetCurrentPadding: {error}"))?;
        let available = buffer_frames.saturating_sub(padding).min(441);
        if available > 0 {
            let buffer = unsafe { render.GetBuffer(available) }
                .map_err(|error| format!("GetBuffer: {error}"))?;
            let samples = unsafe {
                std::slice::from_raw_parts_mut(buffer.cast::<i16>(), available as usize * 2)
            };
            for stereo in samples.as_chunks_mut::<2>().0 {
                let phase = frame as f32 * 440.0 * std::f32::consts::TAU / 44_100.0;
                let sample = (phase.sin() * 8_000.0) as i16;
                stereo[0] = sample;
                stereo[1] = sample;
                frame += 1;
            }
            unsafe { render.ReleaseBuffer(available, 0) }
                .map_err(|error| format!("ReleaseBuffer: {error}"))?;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    unsafe { client.Stop() }.map_err(|error| format!("Stop: {error}"))
}
