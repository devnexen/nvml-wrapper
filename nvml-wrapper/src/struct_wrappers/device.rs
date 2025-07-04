use crate::bitmasks::device::FbcFlags;
use crate::enum_wrappers::device::{
    BridgeChip, Clock, EncoderType, FbcSessionType, PerformanceState, SampleValueType,
};
use crate::enums::device::{FirmwareVersion, SampleValue, UsedGpuMemory};
use crate::error::{nvml_try, Bits, NvmlError};
use crate::ffi::bindings::*;
use crate::structs::device::FieldId;
#[cfg(feature = "serde")]
use serde_derive::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    ffi::{CStr, CString},
};
use std::{
    convert::{TryFrom, TryInto},
    os::raw::c_char,
};

/// PCI information about a GPU device.
// Checked against local
// Tested
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PciInfo {
    /// The bus on which the device resides, 0 to 0xff.
    pub bus: u32,
    /// The PCI identifier.
    pub bus_id: String,
    /// The device's ID on the bus, 0 to 31.
    pub device: u32,
    /// The PCI domain on which the device's bus resides, 0 to 0xffff.
    pub domain: u32,
    /// The combined 16-bit device ID and 16-bit vendor ID.
    pub pci_device_id: u32,
    /**
    The 32-bit Sub System Device ID.

    Will always be `None` if this `PciInfo` was obtained from `NvLink.remote_pci_info()`.
    NVIDIA says that the C field that this corresponds to "is not filled ... and
    is indeterminate" when being returned from that specific call.

    Will be `Some` in all other cases.
    */
    pub pci_sub_system_id: Option<u32>,
}

impl PciInfo {
    /**
    Try to create this struct from its C equivalent.

    Passing `false` for `sub_sys_id_present` will set the `pci_sub_system_id`
    field to `None`. See the field docs for more.

    # Errors

    * `Utf8Error`, if the string obtained from the C function is not valid Utf8
    */
    pub fn try_from(struct_: nvmlPciInfo_t, sub_sys_id_present: bool) -> Result<Self, NvmlError> {
        unsafe {
            let bus_id_raw = CStr::from_ptr(struct_.busId.as_ptr());

            Ok(Self {
                bus: struct_.bus,
                bus_id: bus_id_raw.to_str()?.into(),
                device: struct_.device,
                domain: struct_.domain,
                pci_device_id: struct_.pciDeviceId,
                pci_sub_system_id: if sub_sys_id_present {
                    Some(struct_.pciSubSystemId)
                } else {
                    None
                },
            })
        }
    }
}

impl TryInto<nvmlPciInfo_t> for PciInfo {
    type Error = NvmlError;

    /**
    Convert this `PciInfo` back into its C equivalent.

    # Errors

    * `NulError`, if a nul byte was found in the bus_id (shouldn't occur?)
    * `StringTooLong`, if `bus_id.len()` exceeded the length of
      `NVML_DEVICE_PCI_BUS_ID_BUFFER_SIZE`. This should (?) only be able to
      occur if the user modifies `bus_id` in some fashion. We return an error
      rather than panicking.
    */
    fn try_into(self) -> Result<nvmlPciInfo_t, Self::Error> {
        // This is more readable than spraying `buf_size as usize` everywhere
        const fn buf_size() -> usize {
            NVML_DEVICE_PCI_BUS_ID_BUFFER_SIZE as usize
        }

        let mut bus_id_c: [c_char; buf_size()] = [0; buf_size()];
        let mut bus_id = CString::new(self.bus_id)?.into_bytes_with_nul();

        // Make the string the same length as the array we need to clone it to
        match bus_id.len().cmp(&buf_size()) {
            Ordering::Less => {
                while bus_id.len() != buf_size() {
                    bus_id.push(0);
                }
            }
            Ordering::Equal => {
                // No need to do anything; the buffers are already the same length
            }
            Ordering::Greater => {
                return Err(NvmlError::StringTooLong {
                    max_len: buf_size(),
                    actual_len: bus_id.len(),
                })
            }
        }

        bus_id_c.clone_from_slice(&bus_id.into_iter().map(|b| b as c_char).collect::<Vec<_>>());

        Ok(nvmlPciInfo_t {
            busIdLegacy: [0; NVML_DEVICE_PCI_BUS_ID_BUFFER_V2_SIZE as usize],
            domain: self.domain,
            bus: self.bus,
            device: self.device,
            pciDeviceId: self.pci_device_id,
            // This seems the most correct thing to do? Since this should only
            // be none if obtained from `NvLink.remote_pci_info()`.
            pciSubSystemId: self.pci_sub_system_id.unwrap_or(0),
            busId: bus_id_c,
        })
    }
}

/// BAR1 memory allocation information for a device (in bytes)
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BAR1MemoryInfo {
    /// Unallocated
    pub free: u64,
    /// Total memory
    pub total: u64,
    /// Allocated
    pub used: u64,
}

impl From<nvmlBAR1Memory_t> for BAR1MemoryInfo {
    fn from(struct_: nvmlBAR1Memory_t) -> Self {
        Self {
            free: struct_.bar1Free,
            total: struct_.bar1Total,
            used: struct_.bar1Used,
        }
    }
}

/// Information about a bridge chip.
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BridgeChipInfo {
    pub fw_version: FirmwareVersion,
    pub chip_type: BridgeChip,
}

impl TryFrom<nvmlBridgeChipInfo_t> for BridgeChipInfo {
    type Error = NvmlError;

    /**
    Construct `BridgeChipInfo` from the corresponding C struct.

    # Errors

    * `UnexpectedVariant`, for which you can read the docs for
    */
    fn try_from(value: nvmlBridgeChipInfo_t) -> Result<Self, Self::Error> {
        let fw_version = FirmwareVersion::from(value.fwVersion);
        let chip_type = BridgeChip::try_from(value.type_)?;

        Ok(Self {
            fw_version,
            chip_type,
        })
    }
}

/**
This struct stores the complete hierarchy of the bridge chip within the board.

The immediate bridge is stored at index 0 of `chips_hierarchy`. The parent to
the immediate bridge is at index 1, and so forth.
*/
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BridgeChipHierarchy {
    /// Hierarchy of bridge chips on the board.
    pub chips_hierarchy: Vec<BridgeChipInfo>,
    /// Number of bridge chips on the board.
    pub chip_count: u8,
}

impl TryFrom<nvmlBridgeChipHierarchy_t> for BridgeChipHierarchy {
    type Error = NvmlError;

    /**
    Construct `BridgeChipHierarchy` from the corresponding C struct.

    # Errors

    * `UnexpectedVariant`, for which you can read the docs for
    */
    fn try_from(value: nvmlBridgeChipHierarchy_t) -> Result<Self, Self::Error> {
        let chips_hierarchy = value
            .bridgeChipInfo
            .iter()
            .map(|bci| BridgeChipInfo::try_from(*bci))
            .collect::<Result<_, NvmlError>>()?;

        Ok(Self {
            chips_hierarchy,
            chip_count: value.bridgeCount,
        })
    }
}

/// Information about compute processes running on the GPU.
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ProcessInfo {
    // Process ID.
    pub pid: u32,
    /// Amount of used GPU memory in bytes.
    pub used_gpu_memory: UsedGpuMemory,
    /// The ID of the GPU instance this process is running on, if applicable.
    ///
    /// MIG (Multi-Instance GPU) must be enabled on the device for this field
    /// to be set.
    pub gpu_instance_id: Option<u32>,
    /// The ID of the compute instance this process is running on, if applicable.
    ///
    /// MIG (Multi-Instance GPU) must be enabled on the device for this field
    /// to be set.
    pub compute_instance_id: Option<u32>,
}

impl From<nvmlProcessInfo_t> for ProcessInfo {
    fn from(struct_: nvmlProcessInfo_t) -> Self {
        const NO_VALUE: u32 = 0xFFFFFFFF;

        let gpu_instance_id = Some(struct_.gpuInstanceId).filter(|id| *id != NO_VALUE);
        let compute_instance_id = Some(struct_.computeInstanceId).filter(|id| *id != NO_VALUE);

        Self {
            pid: struct_.pid,
            used_gpu_memory: UsedGpuMemory::from(struct_.usedGpuMemory),
            gpu_instance_id,
            compute_instance_id,
        }
    }
}

/// Detailed ECC error counts for a device.
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EccErrorCounts {
    pub device_memory: u64,
    pub l1_cache: u64,
    pub l2_cache: u64,
    pub register_file: u64,
}

impl From<nvmlEccErrorCounts_t> for EccErrorCounts {
    fn from(struct_: nvmlEccErrorCounts_t) -> Self {
        Self {
            device_memory: struct_.deviceMemory,
            l1_cache: struct_.l1Cache,
            l2_cache: struct_.l2Cache,
            register_file: struct_.registerFile,
        }
    }
}

/// Memory allocation information for a device (in bytes).
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MemoryInfo {
    /// Unallocated FB memory.
    pub free: u64,

    /// Reserved FB memory.
    pub reserved: u64,

    /// Total installed FB memory.
    pub total: u64,
    /// Allocated FB memory.
    ///
    /// Note that the driver/GPU always sets aside a small amount of memory for
    /// bookkeeping.
    pub used: u64,

    /// Struct version, must be set according to API specification before calling the API.
    pub version: u32,
}

impl From<nvmlMemory_v2_t> for MemoryInfo {
    fn from(struct_: nvmlMemory_v2_t) -> Self {
        Self {
            free: struct_.free,
            reserved: struct_.reserved,
            total: struct_.total,
            used: struct_.used,
            version: struct_.version,
        }
    }
}

/// Utilization information for a device. Each sample period may be between 1
/// second and 1/6 second, depending on the product being queried.
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Utilization {
    /// Percent of time over the past sample period during which one or more
    /// kernels was executing on the GPU.
    pub gpu: u32,
    /// Percent of time over the past sample period during which global (device)
    /// memory was being read or written to.
    pub memory: u32,
}

impl From<nvmlUtilization_t> for Utilization {
    fn from(struct_: nvmlUtilization_t) -> Self {
        Self {
            gpu: struct_.gpu,
            memory: struct_.memory,
        }
    }
}

/// Performance policy violation status data.
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ViolationTime {
    /// Represents CPU timestamp in microseconds.
    pub reference_time: u64,
    /// Violation time in nanoseconds.
    pub violation_time: u64,
}

impl From<nvmlViolationTime_t> for ViolationTime {
    fn from(struct_: nvmlViolationTime_t) -> Self {
        Self {
            reference_time: struct_.referenceTime,
            violation_time: struct_.violationTime,
        }
    }
}

/**
Accounting statistics for a process.

There is a field: `unsigned int reserved[5]` present on the C struct that this wraps
that NVIDIA says is "reserved for future use." If it ever gets used in the future,
an equivalent wrapping field will have to be added to this struct.
*/
// Checked against local
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct AccountingStats {
    /**
    Percent of time over the process's lifetime during which one or more kernels was
    executing on the GPU. This is just like what is returned by
    `Device.utilization_rates()` except it is for the lifetime of a process (not just
    the last sample period).

    It will be `None` if `Device.utilization_rates()` is not supported.
    */
    pub gpu_utilization: Option<u32>,
    /// Whether the process is running.
    pub is_running: bool,
    /// Max total memory in bytes that was ever allocated by the process.
    ///
    /// It will be `None` if `ProcessInfo.used_gpu_memory` is not supported.
    pub max_memory_usage: Option<u64>,
    /**
    Percent of time over the process's lifetime during which global (device) memory
    was being read from or written to.

    It will be `None` if `Device.utilization_rates()` is not supported.
    */
    pub memory_utilization: Option<u32>,
    /// CPU timestamp in usec representing the start time for the process.
    pub start_time: u64,
    /// Amount of time in ms during which the compute context was active. This
    /// will be zero if the process is not terminated.
    pub time: u64,
}

impl From<nvmlAccountingStats_t> for AccountingStats {
    fn from(struct_: nvmlAccountingStats_t) -> Self {
        let not_avail_u64 = (NVML_VALUE_NOT_AVAILABLE) as u64;
        let not_avail_u32 = (NVML_VALUE_NOT_AVAILABLE) as u32;

        #[allow(clippy::match_like_matches_macro)]
        Self {
            gpu_utilization: match struct_.gpuUtilization {
                v if v == not_avail_u32 => None,
                _ => Some(struct_.gpuUtilization),
            },
            is_running: match struct_.isRunning {
                0 => false,
                // NVIDIA only says 1 is for running, but I don't think anything
                // else warrants an error (or a panic), so
                _ => true,
            },
            max_memory_usage: match struct_.maxMemoryUsage {
                v if v == not_avail_u64 => None,
                _ => Some(struct_.maxMemoryUsage),
            },
            memory_utilization: match struct_.memoryUtilization {
                v if v == not_avail_u32 => None,
                _ => Some(struct_.memoryUtilization),
            },
            start_time: struct_.startTime,
            time: struct_.time,
        }
    }
}

/// Holds encoder session information.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EncoderSessionInfo {
    /// Unique ID for this session.
    pub session_id: u32,
    /// The ID of the process that owns this session.
    pub pid: u32,
    /// The ID of the vGPU instance that owns this session (if applicable).
    // TODO: Stronger typing if vgpu stuff gets wrapped
    pub vgpu_instance: Option<u32>,
    pub codec_type: EncoderType,
    /// Current horizontal encoding resolution.
    pub hres: u32,
    /// Current vertical encoding resolution.
    pub vres: u32,
    /// Moving average encode frames per second.
    pub average_fps: u32,
    /// Moving average encode latency in μs.
    pub average_latency: u32,
}

impl TryFrom<nvmlEncoderSessionInfo_t> for EncoderSessionInfo {
    type Error = NvmlError;

    /**
    Construct `EncoderSessionInfo` from the corresponding C struct.

    # Errors

    * `UnexpectedVariant`, for which you can read the docs for
    */
    fn try_from(value: nvmlEncoderSessionInfo_t) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: value.sessionId,
            pid: value.pid,
            vgpu_instance: match value.vgpuInstance {
                0 => None,
                other => Some(other),
            },
            codec_type: EncoderType::try_from(value.codecType)?,
            hres: value.hResolution,
            vres: value.vResolution,
            average_fps: value.averageFps,
            average_latency: value.averageLatency,
        })
    }
}

/// Sample info.
// Checked against local
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Sample {
    /// CPU timestamp in μs
    pub timestamp: u64,
    pub value: SampleValue,
}

impl Sample {
    /// Given a tag and an untagged union, returns a Rust enum with the correct
    /// union variant.
    pub fn from_tag_and_struct(tag: &SampleValueType, struct_: nvmlSample_t) -> Self {
        Self {
            timestamp: struct_.timeStamp,
            value: SampleValue::from_tag_and_union(tag, struct_.sampleValue),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ProcessUtilizationSample {
    pub pid: u32,
    /// CPU timestamp in μs
    pub timestamp: u64,
    /// SM (3D / compute) utilization
    pub sm_util: u32,
    /// Frame buffer memory utilization
    pub mem_util: u32,
    /// Encoder utilization
    pub enc_util: u32,
    /// Decoder utilization
    pub dec_util: u32,
}

impl From<nvmlProcessUtilizationSample_t> for ProcessUtilizationSample {
    fn from(struct_: nvmlProcessUtilizationSample_t) -> Self {
        Self {
            pid: struct_.pid,
            timestamp: struct_.timeStamp,
            sm_util: struct_.smUtil,
            mem_util: struct_.memUtil,
            enc_util: struct_.encUtil,
            dec_util: struct_.decUtil,
        }
    }
}

/// Struct that stores information returned from `Device.field_values_for()`.
// TODO: Missing a lot of derives because of the `Result`
#[derive(Debug)]
pub struct FieldValueSample {
    /// The field that this sample is for.
    pub field: FieldId,
    /// This sample's CPU timestamp in μs (Unix time).
    pub timestamp: i64,
    /**
    How long this field value took to update within NVML, in μs.

    This value may be averaged across several fields serviced by the same
    driver call.
    */
    pub latency: i64,
    /// The value of this sample.
    ///
    /// Will be an error if retrieving this specific value failed.
    pub value: Result<SampleValue, NvmlError>,
}

impl TryFrom<nvmlFieldValue_t> for FieldValueSample {
    type Error = NvmlError;

    /**
    Construct `FieldValueSample` from the corresponding C struct.

    # Errors

    * `UnexpectedVariant`, for which you can read the docs for
    */
    fn try_from(value: nvmlFieldValue_t) -> Result<Self, Self::Error> {
        Ok(Self {
            field: FieldId(value.fieldId),
            timestamp: value.timestamp,
            latency: value.latencyUsec,
            value: match nvml_try(value.nvmlReturn) {
                Ok(_) => Ok(SampleValue::from_tag_and_union(
                    &SampleValueType::try_from(value.valueType)?,
                    value.value,
                )),
                Err(e) => Err(e),
            },
        })
    }
}

/// Holds global frame buffer capture session statistics.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FbcStats {
    /// The total number of sessions
    pub sessions_count: u32,
    /// Moving average of new frames captured per second for all capture sessions
    pub average_fps: u32,
    /// Moving average of new frame capture latency in microseconds for all capture sessions
    pub average_latency: u32,
}

impl From<nvmlFBCStats_t> for FbcStats {
    fn from(struct_: nvmlFBCStats_t) -> Self {
        Self {
            sessions_count: struct_.sessionsCount,
            average_fps: struct_.averageFPS,
            average_latency: struct_.averageLatency,
        }
    }
}

/// Information about a frame buffer capture session.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FbcSessionInfo {
    /// Unique session ID
    pub session_id: u32,
    /// The ID of the process that owns this session
    pub pid: u32,
    /// The ID of the vGPU instance that owns this session (if applicable).
    // TODO: Stronger typing if vgpu stuff gets wrapped
    pub vgpu_instance: Option<u32>,
    /// The identifier of the display this session is running on
    pub display_ordinal: u32,
    /// The type of this session
    pub session_type: FbcSessionType,
    /// Various flags with info
    pub session_flags: FbcFlags,
    /// The maximum horizontal resolution supported by this session
    pub hres_max: u32,
    /// The maximum vertical resolution supported by this session
    pub vres_max: u32,
    /// The horizontal resolution requested by the caller in the capture call
    pub hres: u32,
    /// The vertical resolution requested by the caller in the capture call
    pub vres: u32,
    /// Moving average of new frames captured per second for this session
    pub average_fps: u32,
    /// Moving average of new frame capture latency in microseconds for this session
    pub average_latency: u32,
}

impl TryFrom<nvmlFBCSessionInfo_t> for FbcSessionInfo {
    type Error = NvmlError;

    /**
    Construct `FbcSessionInfo` from the corresponding C struct.

    # Errors

    * `UnexpectedVariant`, for which you can read the docs for
    * `IncorrectBits`, if the `sessionFlags` from the given struct do match the
      wrapper definition
    */
    fn try_from(value: nvmlFBCSessionInfo_t) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: value.sessionId,
            pid: value.pid,
            vgpu_instance: match value.vgpuInstance {
                0 => None,
                other => Some(other),
            },
            display_ordinal: value.displayOrdinal,
            session_type: FbcSessionType::try_from(value.sessionType)?,
            session_flags: FbcFlags::from_bits(value.sessionFlags)
                .ok_or(NvmlError::IncorrectBits(Bits::U32(value.sessionFlags)))?,
            hres_max: value.hMaxResolution,
            vres_max: value.vMaxResolution,
            hres: value.hResolution,
            vres: value.vResolution,
            average_fps: value.averageFPS,
            average_latency: value.averageLatency,
        })
    }
}

/// Hardware level attributes from a GPU device
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DeviceAttributes {
    /// Streaming MultiProcessor Count
    pub multiprocessor_count: u32,
    /// Shared Copy Engine Count
    pub shared_copy_engine_count: u32,
    /// Shared Decoder Count
    pub shared_decoder_count: u32,
    /// Shared Encoder Count
    pub shared_encoder_count: u32,
    /// Shared JPEG Count
    pub shared_jpeg_count: u32,
    /// Shared OFA Count
    pub shared_ofa_count: u32,
    /// GPU instance slice Count
    pub gpu_instance_slice_count: u32,
    /// Compute Instance slice count
    pub compute_instance_slice_count: u32,
    /// Device memory size in MB
    pub memory_size_mb: u64,
}

impl From<nvmlDeviceAttributes_t> for DeviceAttributes {
    fn from(struct_: nvmlDeviceAttributes_t) -> Self {
        Self {
            multiprocessor_count: struct_.multiprocessorCount,
            shared_copy_engine_count: struct_.sharedCopyEngineCount,
            shared_decoder_count: struct_.sharedDecoderCount,
            shared_encoder_count: struct_.sharedEncoderCount,
            shared_jpeg_count: struct_.sharedJpegCount,
            shared_ofa_count: struct_.sharedOfaCount,
            gpu_instance_slice_count: struct_.gpuInstanceSliceCount,
            compute_instance_slice_count: struct_.computeInstanceSliceCount,
            memory_size_mb: struct_.memorySizeMB,
        }
    }
}

/// Fan speed info
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FanSpeedInfo {
    /// The API version number
    pub version: u32,
    /// The fan index
    pub fan: u32,
    /// OUT: the fan speed in RPM.
    pub speed: u32,
}

impl From<nvmlFanSpeedInfo_t> for FanSpeedInfo {
    fn from(struct_: nvmlFanSpeedInfo_t) -> Self {
        Self {
            version: struct_.version,
            fan: struct_.fan,
            speed: struct_.speed,
        }
    }
}

/// Clock offset info.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClockOffset {
    /// The API version number
    pub version: u32,
    pub clock_type: Clock,
    pub state: PerformanceState,
    pub clock_offset_mhz: i32,
    pub min_clock_offset_mhz: i32,
    pub max_clock_offset_mhz: i32,
}

impl TryFrom<nvmlClockOffset_v1_t> for ClockOffset {
    type Error = NvmlError;

    fn try_from(value: nvmlClockOffset_v1_t) -> Result<Self, Self::Error> {
        Ok(Self {
            version: value.version,
            clock_type: Clock::try_from(value.type_)?,
            state: PerformanceState::try_from(value.pstate)?,
            clock_offset_mhz: value.clockOffsetMHz,
            min_clock_offset_mhz: value.minClockOffsetMHz,
            max_clock_offset_mhz: value.maxClockOffsetMHz,
        })
    }
}

/// MIG profile placements
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GpuInstancePlacement {
    /// Memory slices occupied
    pub size: u32,
    /// Index of first occupied memory slice (inclusive)
    pub start: u32,
}

impl From<nvmlGpuInstancePlacement_t> for GpuInstancePlacement {
    fn from(value: nvmlGpuInstancePlacement_t) -> Self {
        Self {
            size: value.size,
            start: value.start,
        }
    }
}

// Vgpu
/// Vgpu scheduler capabilities
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerCapabilities {
    // Adaptative Round Robin mode on/off
    pub is_arr_mode_supported: bool,
    // Maximum averaging factor for Adaptative Round Robin mode
    pub max_avg_factor_for_arr: u32,
    // Maximum frequency for Adaptative Round Robin mode
    pub max_freq_for_arr: u32,
    // Maximum timeslice value in ns
    pub max_time_slice: u32,
    // Minimum averaging factor for Adaptative Round Robin mode
    pub min_avg_factor_for_arr: u32,
    // Minimum frequency for Adaptative Round Robin mode
    pub min_freq_for_arr: u32,
    // Minimum timeslice value in ns
    pub min_time_slice: u32,
    // List of supported scheduler
    pub supported_schedulers: Vec<u32>,
}

impl From<nvmlVgpuSchedulerCapabilities_t> for VgpuSchedulerCapabilities {
    fn from(value: nvmlVgpuSchedulerCapabilities_t) -> Self {
        let supported_schedulers = value.supportedSchedulers.to_vec();
        Self {
            is_arr_mode_supported: value.isArrModeSupported > 0,
            max_avg_factor_for_arr: value.maxAvgFactorForARR,
            max_freq_for_arr: value.maxFrequencyForARR,
            max_time_slice: value.maxTimeslice,
            min_avg_factor_for_arr: value.minAvgFactorForARR,
            min_freq_for_arr: value.minFrequencyForARR,
            min_time_slice: value.minTimeslice,
            supported_schedulers,
        }
    }
}

/// Vgpu versions range
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuVersion {
    /// Minimum vGPU version
    pub min: u32,
    /// Maximum vGPU version
    pub max: u32,
}

impl From<nvmlVgpuVersion_t> for VgpuVersion {
    fn from(value: nvmlVgpuVersion_t) -> Self {
        Self {
            min: value.minVersion,
            max: value.maxVersion,
        }
    }
}

impl VgpuVersion {
    pub fn as_c(&self) -> nvmlVgpuVersion_t {
        nvmlVgpuVersion_t {
            minVersion: self.min,
            maxVersion: self.max,
        }
    }
}

/// Vgpu scheduler Params
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerParams {
    pub avg_factor: Option<u32>,
    pub timeslice: u32,
}

/// Vgpu scheduler Log entry
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerLogEntry {
    /// Timesteamp when the software runlist was preemopted (in ns)
    pub timestamp: u64,
    /// Total time this runlist has run (in ns)
    pub time_run_total: u64,
    /// Time this runlist ran before preemption (in ns)
    pub time_run: u64,
    /// Runlist Id
    pub sw_runlist_id: u32,
    /// timeslice after deduction
    pub target_time_slice: u64,
    /// Preemption time for this runlist (in ns)
    pub cumulative_preemption_time: u64,
}

impl From<nvmlVgpuSchedulerLogEntry_t> for VgpuSchedulerLogEntry {
    fn from(value: nvmlVgpuSchedulerLogEntry_t) -> Self {
        Self {
            timestamp: value.timestamp,
            time_run_total: value.timeRunTotal,
            time_run: value.timeRun,
            sw_runlist_id: value.swRunlistId,
            target_time_slice: value.targetTimeSlice,
            cumulative_preemption_time: value.cumulativePreemptionTime,
        }
    }
}

/// Vgpu scheduler Log
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerLog {
    /// Engine id whose software runlist are fetched
    pub engine_id: u32,
    /// Scheduler policy
    pub scheduler_policy: u32,
    /// Scheduler Round Robin Mode
    pub arr_mode: u32,
    pub scheduler_params: VgpuSchedulerParams,
    /// Number of log entries fetched during the call
    pub entries_count: u32,
    /// Log entries
    pub entries: Vec<VgpuSchedulerLogEntry>,
}

impl From<nvmlVgpuSchedulerLog_t> for VgpuSchedulerLog {
    fn from(value: nvmlVgpuSchedulerLog_t) -> Self {
        let entries = value
            .logEntries
            .iter()
            .map(|e| VgpuSchedulerLogEntry::from(*e))
            .collect::<Vec<_>>();
        let params = match value.arrMode {
            2 => {
                let data = unsafe { value.schedulerParams.vgpuSchedDataWithARR };
                VgpuSchedulerParams {
                    avg_factor: Some(data.avgFactor),
                    timeslice: data.timeslice,
                }
            }
            _ => {
                let data = unsafe { value.schedulerParams.vgpuSchedData };
                VgpuSchedulerParams {
                    avg_factor: None,
                    timeslice: data.timeslice,
                }
            }
        };

        Self {
            engine_id: value.engineId,
            scheduler_policy: value.schedulerPolicy,
            arr_mode: value.arrMode,
            scheduler_params: params,
            entries_count: entries.len() as u32,
            entries,
        }
    }
}

/// Vgpu scheduler state
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerGetState {
    /// Adaptative Round Robin scheduler mode
    pub arr_mode: u32,
    /// Scheduler policy
    pub scheduler_policy: u32,
}

impl From<nvmlVgpuSchedulerGetState_t> for VgpuSchedulerGetState {
    fn from(value: nvmlVgpuSchedulerGetState_t) -> Self {
        Self {
            arr_mode: value.arrMode,
            scheduler_policy: value.schedulerPolicy,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerSetParams {
    /// Average factor in compensating the timeslice for Adaptive Round Robin mode
    pub avg_factor: Option<u32>,
    /// Frequency for Adaptative Mode (when avg_factor is set)/timeslice in ns for each software run list as configured, or the default value otherwise
    pub frequency_or_timeslice: u32,
}

impl VgpuSchedulerSetParams {
    pub fn as_c(&self) -> nvmlVgpuSchedulerSetParams_t {
        match self.avg_factor {
            Some(a) => nvmlVgpuSchedulerSetParams_t {
                vgpuSchedDataWithARR: nvmlVgpuSchedulerSetParams_t__bindgen_ty_1 {
                    avgFactor: a,
                    frequency: self.frequency_or_timeslice,
                },
            },
            _ => nvmlVgpuSchedulerSetParams_t {
                vgpuSchedData: nvmlVgpuSchedulerSetParams_t__bindgen_ty_2 {
                    timeslice: self.frequency_or_timeslice,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VgpuSchedulerSetState {
    pub scheduler_policy: u32,
    pub enable_arr_mode: u32,
    pub scheduler_params: VgpuSchedulerSetParams,
}

impl VgpuSchedulerSetState {
    pub fn as_c(&self) -> nvmlVgpuSchedulerSetState_t {
        nvmlVgpuSchedulerSetState_t {
            enableARRMode: self.enable_arr_mode,
            schedulerPolicy: self.scheduler_policy,
            schedulerParams: self.scheduler_params.as_c(),
        }
    }
}

#[cfg(test)]
#[allow(unused_variables, unused_imports)]
mod tests {
    use crate::error::*;
    use crate::ffi::bindings::*;
    use crate::test_utils::*;
    use std::convert::TryInto;
    use std::mem;

    #[test]
    fn pci_info_from_to_c() {
        let nvml = nvml();
        test_with_device(3, &nvml, |device| {
            let converted: nvmlPciInfo_t = device
                .pci_info()
                .expect("wrapped pci info")
                .try_into()
                .expect("converted c pci info");

            let sym = nvml_sym(nvml.lib.nvmlDeviceGetPciInfo_v3.as_ref())?;

            let raw = unsafe {
                let mut pci_info: nvmlPciInfo_t = mem::zeroed();
                nvml_try(sym(device.handle(), &mut pci_info)).expect("raw pci info");
                pci_info
            };

            assert_eq!(converted.busId, raw.busId);
            assert_eq!(converted.domain, raw.domain);
            assert_eq!(converted.bus, raw.bus);
            assert_eq!(converted.device, raw.device);
            assert_eq!(converted.pciDeviceId, raw.pciDeviceId);
            assert_eq!(converted.pciSubSystemId, raw.pciSubSystemId);

            Ok(())
        })
    }
}
