use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

pub fn initialize() -> Result<(), String> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| error.to_string())
}

pub fn default_render_name() -> Result<String, String> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| error.to_string())?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|error| error.to_string())?;
    let properties =
        unsafe { device.OpenPropertyStore(STGM_READ) }.map_err(|error| error.to_string())?;
    let name = unsafe { properties.GetValue(&PKEY_Device_FriendlyName) }
        .map_err(|error| error.to_string())?;
    Ok(name.to_string())
}
