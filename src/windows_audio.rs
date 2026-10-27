use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
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

/// Mirror a receiver volume change to the Windows slider. A receiver volume
/// notification has no independent mute bit, so it must not change endpoint
/// mute state (which Windows and USB Audio manage separately).
pub fn set_render_volume(expected_name: &str, scalar: f32) -> Result<(), String> {
    let device = named_render_output(expected_name)?;
    let endpoint_volume: IAudioEndpointVolume =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let current = unsafe { endpoint_volume.GetMasterVolumeLevelScalar() }
        .map_err(|error| error.to_string())?;
    if (current - scalar).abs() >= 0.005 {
        unsafe { endpoint_volume.SetMasterVolumeLevelScalar(scalar, std::ptr::null()) }
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn named_render_output(expected_name: &str) -> Result<IMMDevice, String> {
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
        if name == format!("Speakers ({expected_name})") {
            chosen = Some(device);
            break;
        }
    }
    chosen.ok_or_else(|| format!("Audio endpoint containing {expected_name:?} was not found"))
}

/// Read the percentage and mute state Windows displays for a virtual output.
/// USB Feature Unit values are decibels and do not match this slider scale.
pub fn render_output_volume(expected_name: &str) -> Result<crate::airplay::Volume, String> {
    let device = named_render_output(expected_name)?;
    let endpoint: IAudioEndpointVolume =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let scalar =
        unsafe { endpoint.GetMasterVolumeLevelScalar() }.map_err(|error| error.to_string())?;
    let muted = unsafe { endpoint.GetMute() }
        .map_err(|error| error.to_string())?
        .as_bool();
    Ok(crate::airplay::Volume { scalar, muted })
}

/// Diagnostic value exposed by WASAPI for an initialized render stream.
pub fn render_stream_latency_ms(expected_name: &str) -> Result<f64, String> {
    let device = named_render_output(expected_name)?;
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
        .map_err(|error| format!("WASAPI initialization failed: {error}"))?;
    let latency = unsafe { client.GetStreamLatency() }.map_err(|error| error.to_string())?;
    Ok(latency as f64 / 10_000.0)
}

/// Render through a named endpoint without changing the system default output.
pub fn play_test_tone(expected_name: &str, duration: std::time::Duration) -> Result<(), String> {
    let device = named_render_output(expected_name)?;
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

/// Exercise the endpoint controls used by the Windows sound picker.
pub fn test_endpoint_controls(expected_name: &str) -> Result<(), String> {
    let device = named_render_output(expected_name)?;
    let endpoint: IAudioEndpointVolume =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| error.to_string())?;
    let original_master =
        unsafe { endpoint.GetMasterVolumeLevelScalar() }.map_err(|error| error.to_string())?;
    let original_mute = unsafe { endpoint.GetMute() }.map_err(|error| error.to_string())?;
    let channels = unsafe { endpoint.GetChannelCount() }.map_err(|error| error.to_string())?;
    if channels < 2 {
        return Err(format!(
            "Expected stereo volume controls; found {channels} channels"
        ));
    }
    let original_left =
        unsafe { endpoint.GetChannelVolumeLevelScalar(0) }.map_err(|error| error.to_string())?;
    let result = (|| {
        unsafe { endpoint.SetMasterVolumeLevelScalar(0.4, std::ptr::null()) }
            .map_err(|error| format!("SetMasterVolumeLevelScalar: {error}"))?;
        let master =
            unsafe { endpoint.GetMasterVolumeLevelScalar() }.map_err(|error| error.to_string())?;
        if (master - 0.4).abs() > 0.05 {
            return Err(format!("Master volume did not change: {master}"));
        }
        unsafe { endpoint.SetChannelVolumeLevelScalar(0, 0.3, std::ptr::null()) }
            .map_err(|error| format!("SetChannelVolumeLevelScalar: {error}"))?;
        let left = unsafe { endpoint.GetChannelVolumeLevelScalar(0) }
            .map_err(|error| error.to_string())?;
        if (left - 0.3).abs() > 0.05 {
            return Err(format!("Left channel volume did not change: {left}"));
        }
        unsafe { endpoint.SetMute(BOOL::from(true), std::ptr::null()) }
            .map_err(|error| format!("SetMute: {error}"))?;
        if !unsafe { endpoint.GetMute() }
            .map_err(|error| error.to_string())?
            .as_bool()
        {
            return Err("Mute control did not change".into());
        }
        set_render_volume(expected_name, 0.7)?;
        if !unsafe { endpoint.GetMute() }
            .map_err(|error| error.to_string())?
            .as_bool()
        {
            return Err("Receiver volume update unexpectedly cleared Windows mute".into());
        }
        unsafe { endpoint.SetMute(BOOL::from(false), std::ptr::null()) }
            .map_err(|error| format!("Clear mute: {error}"))?;
        set_render_volume(expected_name, 0.0)?;
        if unsafe { endpoint.GetMute() }
            .map_err(|error| error.to_string())?
            .as_bool()
        {
            return Err("Receiver volume update unexpectedly muted Windows".into());
        }
        Ok(())
    })();
    let _ = unsafe { endpoint.SetMute(original_mute, std::ptr::null()) };
    let _ = unsafe { endpoint.SetChannelVolumeLevelScalar(0, original_left, std::ptr::null()) };
    let _ = unsafe { endpoint.SetMasterVolumeLevelScalar(original_master, std::ptr::null()) };
    result
}
