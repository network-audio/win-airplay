use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
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
