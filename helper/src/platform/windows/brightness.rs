use super::monitor::monitor_device_id;
use crate::platform::adjustment::Level;
use crate::platform::brightness::adjusted_brightness;
use crate::platform::Monitor;
use anyhow::{ensure, Context, Result};
use windows::core::{w, BSTR, PCWSTR, VARIANT};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{POINT, RPC_E_TOO_LATE};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, HMONITOR, MONITORINFOEXW, MONITOR_DEFAULTTONULL,
};
use windows::Win32::Security::PSECURITY_DESCRIPTOR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoInitializeSecurity, CoSetProxyBlanket, CoUninitialize,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WbemLocator, WBEM_FLAG_CONNECT_USE_MAX_WAIT,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_FLAG_RETURN_WBEM_COMPLETE,
};

pub(crate) fn adjust_monitor_brightness(target: &Monitor, delta: f32) -> Result<Level> {
    let monitor = super::monitors()?
        .into_iter()
        .find(|monitor| monitor.id() == target.id())
        .context("目标显示器已断开")?;
    let handle = unsafe {
        MonitorFromPoint(
            POINT {
                x: monitor.bounds.left + monitor.bounds.width() / 2,
                y: monitor.bounds.top + monitor.bounds.height() / 2,
            },
            MONITOR_DEFAULTTONULL,
        )
    };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    ensure!(
        unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) }.as_bool()
            && monitor_device_id(&info.szDevice) == monitor.device_id,
        "目标显示器已变更"
    );

    // 内屏 WMI 只是候选后端，不是前置条件：台式机上 WmiMonitorBrightness 类存在但查询返回
    // “不支持”，此时必须继续落到 DDC/CI，否则只有外接屏的机器完全调不动亮度。
    resolve_backend(adjust_internal(&monitor.device_id, delta), || {
        adjust_external(handle, delta)
    })
}

/// 内屏后端的三种结果对应三种处理：拿到读数就用它；没匹配到内屏就交给 DDC/CI；
/// **内屏后端报错也必须继续尝试 DDC/CI**，只在两者都失败时才把内屏原因附到最终报错上。
fn resolve_backend<T>(
    internal: Result<Option<T>>,
    external: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match internal {
        Ok(Some(level)) => Ok(level),
        Ok(None) => external(),
        Err(internal_error) => external()
            .map_err(|error| error.context(format!("Windows 亮度接口：{internal_error:#}"))),
    }
}

struct PhysicalMonitors(Vec<PHYSICAL_MONITOR>);

impl Drop for PhysicalMonitors {
    fn drop(&mut self) {
        let _ = unsafe { DestroyPhysicalMonitors(&self.0) };
    }
}

fn adjust_external(monitor: HMONITOR, delta: f32) -> Result<Level> {
    let mut count = 0;
    unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count)? };
    ensure!(count > 0, "显示器未提供亮度控制");
    let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
    unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical)? };
    let physical = PhysicalMonitors(physical);
    // 同一个 HMONITOR 下有多个物理面板时全部调整（与旧行为一致），读数取第一个。
    let mut level = None;
    for monitor in &physical.0 {
        let (mut min, mut current, mut max) = (0, 0, 0);
        ensure!(
            unsafe {
                GetMonitorBrightness(monitor.hPhysicalMonitor, &mut min, &mut current, &mut max)
            } != 0,
            "显示器未提供亮度控制，请检查 DDC/CI 设置"
        );
        let next = adjusted_brightness(min, current, max, delta)?;
        if next != current {
            ensure!(
                unsafe { SetMonitorBrightness(monitor.hPhysicalMonitor, next) } != 0,
                "显示器拒绝调整亮度"
            );
        }
        // 写入成功就算生效：回读失败时用请求值兜底，不要把已经改掉的亮度报成失败。
        let mut reported = next;
        let (mut read_min, mut read_current, mut read_max) = (0, 0, 0);
        if unsafe {
            GetMonitorBrightness(
                monitor.hPhysicalMonitor,
                &mut read_min,
                &mut read_current,
                &mut read_max,
            )
        } != 0
        {
            reported = read_current;
        }
        if level.is_none() {
            let description = monitor.szPhysicalMonitorDescription;
            let end = description.iter().position(|c| *c == 0).unwrap_or(128);
            level = Some(Level::brightness(
                min,
                reported,
                max,
                String::from_utf16_lossy(&description[..end]),
            )?);
        }
    }
    level.context("显示器未提供亮度控制")
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn device_instance(device_id: &[u16]) -> Option<String> {
    let length = device_id
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(device_id.len());
    let path = String::from_utf16_lossy(&device_id[..length]);
    let mut parts = path.strip_prefix(r"\\?\")?.split('#');
    let class = parts.next()?;
    let model = parts.next()?;
    let instance = parts.next()?;
    if !class.eq_ignore_ascii_case("DISPLAY") || model.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!("{class}\\{model}\\{instance}").to_ascii_lowercase())
}

fn matches_instance(target: &str, instance: &str) -> bool {
    let instance = instance.to_ascii_lowercase();
    instance == target
        || instance.strip_prefix(target).is_some_and(|suffix| {
            suffix.strip_prefix('_').is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

fn property(object: &IWbemClassObject, name: PCWSTR) -> Result<VARIANT> {
    let mut value = VARIANT::default();
    unsafe { object.Get(name, 0, &mut value, None, None)? };
    Ok(value)
}

fn adjust_internal(device_id: &[u16], delta: f32) -> Result<Option<Level>> {
    let Some(target) = device_instance(device_id) else {
        return Ok(None);
    };
    // Propagate WMI initialization failures before selecting a brightness backend.
    let apartment = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if apartment.is_err() {
        return Err(anyhow::anyhow!("Windows COM 初始化失败：{apartment:?}"));
    }
    let _apartment = ComApartment;
    let security = unsafe {
        CoInitializeSecurity(
            PSECURITY_DESCRIPTOR::default(),
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_DEFAULT,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
            None,
        )
    };
    if let Err(error) = security {
        if error.code() != RPC_E_TOO_LATE {
            return Err(error.into());
        }
    }
    let locator: IWbemLocator =
        unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)? };
    let services = unsafe {
        locator.ConnectServer(
            &BSTR::from(r"ROOT\WMI"),
            None,
            None,
            None,
            WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
            None,
            None,
        )?
    };
    unsafe {
        CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?
    };
    let records = unsafe {
        services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT * FROM WmiMonitorBrightness WHERE Active = TRUE"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?
    };
    loop {
        let mut objects = [None];
        let mut count = 0;
        unsafe { records.Next(2000, &mut objects, &mut count).ok()? };
        let Some(object) = objects[0].take() else {
            return Ok(None);
        };
        let instance = BSTR::try_from(&property(&object, w!("InstanceName"))?)?.to_string();
        if !matches_instance(&target, &instance) {
            continue;
        }
        let current = u32::try_from(&property(&object, w!("CurrentBrightness"))?)?;
        let next = adjusted_brightness(0, current, 100, delta)?;
        if next == current {
            return Ok(Some(Level::brightness(0, current, 100, "内置显示器")?));
        }

        let mut class = None;
        unsafe {
            services.GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                Some(&mut class),
                None,
            )?
        };
        let class = class.context("Windows 亮度方法不可用")?;
        let mut signature = None;
        unsafe {
            class.GetMethod(
                w!("WmiSetBrightness"),
                0,
                &mut signature,
                std::ptr::null_mut(),
            )?
        };
        let input = unsafe {
            signature
                .context("Windows 亮度参数不可用")?
                .SpawnInstance(0)?
        };
        unsafe {
            input.Put(w!("Timeout"), 0, &VARIANT::from(0i32), 0)?;
            input.Put(w!("Brightness"), 0, &VARIANT::from(next as u8), 0)?;
        }
        let path = format!(
            "WmiMonitorBrightnessMethods.InstanceName=\"{}\"",
            instance.replace('\\', "\\\\").replace('"', "\\\"")
        );
        unsafe {
            services.ExecMethod(
                &BSTR::from(path),
                &BSTR::from("WmiSetBrightness"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                &input,
                None,
                None,
            )?
        };
        let object_path = BSTR::try_from(&property(&object, w!("__PATH"))?)?;
        let mut updated = None;
        unsafe {
            services.GetObject(
                &object_path,
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                Some(&mut updated),
                None,
            )?;
        }
        let current = u32::try_from(&property(
            &updated.context("亮度写入后读取失败")?,
            w!("CurrentBrightness"),
        )?)?;
        return Ok(Some(Level::brightness(0, current, 100, "内置显示器")?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal_failure_still_falls_back_to_ddc_ci() {
        // 回归护栏：0.6.2 合并 PR #14 时内屏后端改成 `?` 直接返回错误，
        // 台式机上 WmiMonitorBrightness 查询“不支持”就让外接屏完全调不动亮度。
        let mut attempted = false;
        let level = resolve_backend::<u32>(Err(anyhow::anyhow!("WMI 不支持")), || {
            attempted = true;
            Ok(42)
        })
        .unwrap();
        assert!(attempted, "内屏后端报错后必须继续尝试 DDC/CI");
        assert_eq!(level, 42);

        // 两者都失败时保留内屏原因，便于定位到底是哪条通道的问题。
        // anyhow 的 `{:#}` 才会展开整条 cause 链，`to_string()` 只有最外层上下文。
        let error = resolve_backend::<u32>(Err(anyhow::anyhow!("WMI 不支持")), || {
            Err(anyhow::anyhow!("显示器拒绝调整亮度"))
        })
        .unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("显示器拒绝调整亮度"), "{chain}");
        assert!(chain.contains("WMI 不支持"), "{chain}");

        // 命中内屏时不再触碰 DDC/CI，没匹配到内屏时照旧回落。
        let mut external_calls = 0;
        assert_eq!(
            resolve_backend::<u32>(Ok(Some(7)), || {
                external_calls += 1;
                Ok(0)
            })
            .unwrap(),
            7
        );
        assert_eq!(
            resolve_backend::<u32>(Ok(None), || {
                external_calls += 1;
                Ok(9)
            })
            .unwrap(),
            9
        );
        assert_eq!(external_calls, 1);
    }

    #[test]
    fn wmi_matching_distinguishes_identical_monitor_models_and_instance_prefixes() {
        let path: Vec<u16> = r"\\?\DISPLAY#ACME123#5&123456&1&UID100#{guid}"
            .encode_utf16()
            .chain([0])
            .collect();
        let target = device_instance(&path).unwrap();
        assert!(matches_instance(
            &target,
            r"DISPLAY\ACME123\5&123456&1&UID100_0"
        ));
        assert!(!matches_instance(
            &target,
            r"DISPLAY\ACME123\5&123456&1&UID1000_0"
        ));
        assert!(!matches_instance(
            &target,
            r"DISPLAY\ACME123\5&123456&1&UID101_0"
        ));
        assert!(device_instance(&[0; 128]).is_none());
    }
}
