use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
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
    Ok(DefaultOutput {
        name: name.to_string(),
        volume: crate::airplay::Volume { scalar, muted },
    })
}
