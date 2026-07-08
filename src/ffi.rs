use core::ffi::c_char;
use std::ffi::CStr;

use crate::{
    parse_raman_artifact_binary, Bandwidth, NoopRamanPlatform, RamanCSignalInput,
    RamanCSignalOutput, RamanRuntimeArtifact, RamanRuntimeContext, RamanRuntimeHost,
};

struct RamanFfiRuntime {
    host: RamanRuntimeHost<NoopRamanPlatform>,
}

#[unsafe(no_mangle)]
pub extern "C" fn raman_runtime_new_from_binary(
    artifact_ptr: *const u8,
    artifact_len: usize,
) -> *mut core::ffi::c_void {
    let Some(artifact) = artifact_from_binary(artifact_ptr, artifact_len) else {
        return core::ptr::null_mut();
    };
    let Ok(host) = RamanRuntimeHost::from_artifact(NoopRamanPlatform::default(), &artifact) else {
        return core::ptr::null_mut();
    };
    Box::into_raw(Box::new(RamanFfiRuntime { host })).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn raman_runtime_free(runtime: *mut core::ffi::c_void) {
    if runtime.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(runtime.cast::<RamanFfiRuntime>()));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn raman_process_signal(
    runtime: *mut core::ffi::c_void,
    input: *const RamanCSignalInput,
    output: *mut RamanCSignalOutput,
) -> i32 {
    if runtime.is_null() || input.is_null() || output.is_null() {
        return -1;
    }
    let runtime = unsafe { &mut *runtime.cast::<RamanFfiRuntime>() };
    let Some((stream_id, context)) = (unsafe { context_from_ffi(&*input) }) else {
        return -2;
    };
    match runtime.host.process_signal(&stream_id, &context) {
        Ok(report) => {
            unsafe {
                *output = RamanCSignalOutput::default();
                (*output).status_code = 0;
                (*output).applied_at_us = report.applied_at_us;
                (*output).bandwidth_khz = bandwidth_khz(report.decision.snapshot.phy.bandwidth);
                (*output).spreading_factor = report
                    .decision
                    .snapshot
                    .phy
                    .spreading_factor
                    .map(|value| value as u8 + 7)
                    .unwrap_or(0);
                (*output).tx_power_dbm = report.decision.snapshot.phy.tx_power_dbm;
                (*output).temporal_reused = u8::from(report.decision.temporal_reused);
                write_c_string(&mut (*output).priority, &report.decision.priority);
                write_c_string(
                    &mut (*output).profile_name,
                    &report.decision.snapshot.profile_name,
                );
                write_c_string(
                    &mut (*output).transport_priority,
                    report
                        .decision
                        .snapshot
                        .transport_priority
                        .as_deref()
                        .unwrap_or(""),
                );
                write_c_string(
                    &mut (*output).ris_mode,
                    report
                        .decision
                        .snapshot
                        .ris
                        .as_ref()
                        .map(|value| value.mode.as_str())
                        .unwrap_or(""),
                );
            }
            0
        }
        Err(_) => -3,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn raman_process_signal_binary(
    artifact_ptr: *const u8,
    artifact_len: usize,
    input: *const RamanCSignalInput,
    output: *mut RamanCSignalOutput,
) -> i32 {
    let runtime = raman_runtime_new_from_binary(artifact_ptr, artifact_len);
    if runtime.is_null() {
        return -4;
    }
    let status = raman_process_signal(runtime, input, output);
    raman_runtime_free(runtime);
    status
}

fn artifact_from_binary(ptr: *const u8, len: usize) -> Option<RamanRuntimeArtifact> {
    if ptr.is_null() || len == 0 {
        return None;
    }
    let raw = unsafe { core::slice::from_raw_parts(ptr, len) };
    parse_raman_artifact_binary(raw).ok()
}

unsafe fn context_from_ffi(input: &RamanCSignalInput) -> Option<(String, RamanRuntimeContext)> {
    let stream_id = read_c_string(input.stream_id)?;
    Some((
        stream_id,
        RamanRuntimeContext {
            minute: input.minute,
            traffic_class: read_c_string(input.traffic_class)?,
            hardware: read_c_string(input.hardware)?,
            scenario: read_c_string(input.scenario)?,
            noise_floor_dbm: input.noise_floor_dbm,
            snr_db: input.snr_db,
            density: input.density,
            latency_budget_ms: input.latency_budget_ms,
            battery_mv: input.battery_mv,
            link_margin_db: input.link_margin_db,
            confidence: input.confidence,
            predicted: read_c_string(input.predicted)?,
            drift_db: input.drift_db,
            renegotiation_needed: input.renegotiation_needed != 0,
            rx_pdr: input.rx_pdr,
            error_rate: input.error_rate,
            path_stability: input.path_stability,
            memory_confidence: input.memory_confidence,
            worst_hop_reliability: input.worst_hop_reliability,
        },
    ))
}

unsafe fn read_c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok().map(str::to_string)
}

fn write_c_string<const N: usize>(buffer: &mut [c_char; N], value: &str) {
    buffer.fill(0);
    for (idx, byte) in value.bytes().take(N.saturating_sub(1)).enumerate() {
        buffer[idx] = byte as c_char;
    }
}

fn bandwidth_khz(value: Bandwidth) -> u16 {
    match value {
        Bandwidth::Khz62 => 62,
        Bandwidth::Khz125 => 125,
        Bandwidth::Khz250 => 250,
        Bandwidth::Khz500 => 500,
    }
}
