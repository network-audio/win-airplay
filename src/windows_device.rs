use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct DevPropKey {
    fmtid: Guid,
    pid: u32,
}

#[repr(C)]
struct DevPropCompKey {
    key: DevPropKey,
    store: u32,
    locale_name: *const u16,
}

#[repr(C)]
struct DevProperty {
    comp_key: DevPropCompKey,
    kind: u32,
    buffer_size: u32,
    buffer: *const c_void,
}

#[repr(C)]
struct SwDeviceCreateInfo {
    cb_size: u32,
    instance_id: *const u16,
    hardware_ids: *const u16,
    compatible_ids: *const u16,
    container_id: *const Guid,
    capability_flags: u32,
    device_description: *const u16,
    device_location: *const u16,
    security_descriptor: *const c_void,
}

type SwDeviceHandle = *mut c_void;
type CreateResult = mpsc::Sender<Result<(), i32>>;

#[link(name = "Swdevice")]
unsafe extern "system" {
    fn SwDeviceCreate(
        enumerator_name: *const u16,
        parent_device_instance: *const u16,
        create_info: *const SwDeviceCreateInfo,
        property_count: u32,
        properties: *const DevProperty,
        callback: Option<unsafe extern "system" fn(SwDeviceHandle, i32, *mut c_void, *const u16)>,
        context: *mut c_void,
        handle: *mut SwDeviceHandle,
    ) -> i32;
    fn SwDeviceClose(handle: SwDeviceHandle);
}

pub struct SoftwareDevice {
    handle: SwDeviceHandle,
}

unsafe extern "system" fn created(
    _handle: SwDeviceHandle,
    result: i32,
    context: *mut c_void,
    _device_instance_id: *const u16,
) {
    let sender = unsafe { Box::from_raw(context.cast::<CreateResult>()) };
    let _ = sender.send(if result >= 0 { Ok(()) } else { Err(result) });
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

impl SoftwareDevice {
    pub fn create(instance_id: &str, name: &str) -> Result<Self, String> {
        let enumerator = wide("WinAirPlay");
        let parent = wide("HTREE\\ROOT\\0");
        let instance_id = wide(instance_id);
        let hardware_ids = wide("SWD\\WinAirPlay");
        let mut hardware_ids = hardware_ids;
        hardware_ids.push(0);
        let description = wide(name);
        let name_value = wide(name);
        let create_info = SwDeviceCreateInfo {
            cb_size: std::mem::size_of::<SwDeviceCreateInfo>() as u32,
            instance_id: instance_id.as_ptr(),
            hardware_ids: hardware_ids.as_ptr(),
            compatible_ids: ptr::null(),
            container_id: ptr::null(),
            // Removable, silent installation, and a required function driver.
            capability_flags: 0x1 | 0x2 | 0x8,
            device_description: description.as_ptr(),
            device_location: ptr::null(),
            security_descriptor: ptr::null(),
        };
        let property = DevProperty {
            comp_key: DevPropCompKey {
                key: DevPropKey {
                    fmtid: Guid {
                        data1: 0x42dbc20a,
                        data2: 0x8d71,
                        data3: 0x4bec,
                        data4: [0xb3, 0x37, 0xe0, 0x40, 0xb5, 0x29, 0xb1, 0x63],
                    },
                    pid: 2,
                },
                store: 0,
                locale_name: ptr::null(),
            },
            kind: 0x12, // DEVPROP_TYPE_STRING
            buffer_size: (name_value.len() * 2) as u32,
            buffer: name_value.as_ptr().cast(),
        };

        let (sender, receiver) = mpsc::channel::<Result<(), i32>>();
        let context = Box::into_raw(Box::new(sender)).cast();
        let mut handle = ptr::null_mut();
        let result = unsafe {
            SwDeviceCreate(
                enumerator.as_ptr(),
                parent.as_ptr(),
                &create_info,
                1,
                &property,
                Some(created),
                context,
                &mut handle,
            )
        };
        if result < 0 {
            unsafe { drop(Box::from_raw(context.cast::<CreateResult>())) };
            return Err(format!("SwDeviceCreate failed: HRESULT {result:#x}"));
        }
        // Closing the handle also removes the PnP device. A callback that arrives
        // after this timeout still owns and frees its context.
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(())) => Ok(Self { handle }),
            Ok(Err(code)) => {
                unsafe { SwDeviceClose(handle) };
                Err(format!(
                    "software device creation failed: HRESULT {code:#x}"
                ))
            }
            Err(error) => {
                unsafe { SwDeviceClose(handle) };
                Err(format!(
                    "software device creation did not complete: {error}"
                ))
            }
        }
    }
}

impl Drop for SoftwareDevice {
    fn drop(&mut self) {
        unsafe { SwDeviceClose(self.handle) };
    }
}
