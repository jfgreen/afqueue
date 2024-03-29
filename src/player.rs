use std::cmp;
use std::error::Error;
use std::ffi::{c_void, CStr, CString, NulError};
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr;

//TODO: Check for lots of inner loop allocs (i.e Vec::new or vec!)

use crate::ffi::audio_toolbox::{
    self, audio_queue_get_current_time, AudioFileID, AudioQueueBufferRef,
    AudioQueueLevelMeterState, AudioQueuePropertyID, AudioQueueRef, AudioStreamBasicDescription,
    AudioTimeStamp,
};

use crate::events::CallbackNotifier;
use crate::ffi::core_foundation;

pub type PlaybackResult<T> = Result<T, PlaybackError>;

#[derive(Debug)]
pub enum PlaybackError {
    Path(PathError),
    System(SystemErrorCode),
    IO(io::Error),
}

impl From<PathError> for PlaybackError {
    fn from(err: PathError) -> PlaybackError {
        PlaybackError::Path(err)
    }
}

impl From<SystemErrorCode> for PlaybackError {
    fn from(err: SystemErrorCode) -> PlaybackError {
        // TODO: Map common error codes to enum variants
        PlaybackError::System(err)
    }
}

impl From<io::Error> for PlaybackError {
    fn from(err: io::Error) -> PlaybackError {
        PlaybackError::IO(err)
    }
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PlaybackError::Path(err) => {
                write!(f, "supplied string is not a valid path: {err}")
            }
            PlaybackError::System(SystemErrorCode(code)) => {
                write!(f, "encountered system error with code '{code}'")
            }
            PlaybackError::IO(err) => {
                write!(f, "encountered IO error '{err}'")
            }
        }
    }
}

impl Error for PlaybackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PlaybackError::Path(err) => Some(err),
            PlaybackError::System(err) => Some(err),
            PlaybackError::IO(err) => Some(err),
        }
    }
}

#[derive(Debug)]
pub enum PathError {
    InteriorNull(NulError),
    PathIsEmpty,
}

impl From<NulError> for PathError {
    fn from(err: NulError) -> PathError {
        PathError::InteriorNull(err)
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PathError::InteriorNull(err) => {
                write!(f, "Path contained a null: {err}")
            }
            PathError::PathIsEmpty => {
                write!(f, "Attempted to interpret an empty string as a path")
            }
        }
    }
}

impl Error for PathError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PathError::InteriorNull(err) => Some(err),
            PathError::PathIsEmpty => None,
        }
    }
}

type SystemResult<T> = Result<T, SystemErrorCode>;

#[derive(Debug)]
pub struct SystemErrorCode(i32);

impl fmt::Display for SystemErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let SystemErrorCode(code) = self;
        write!(f, "{code}")
    }
}

impl Error for SystemErrorCode {}

type PacketPosition = i64;
type PacketCount = u32;

const LOWER_BUFFER_SIZE_HINT: u32 = 0x4000;
const UPPER_BUFFER_SIZE_HINT: u32 = 0x50000;
const BUFFER_SECONDS_HINT: f64 = 0.5;
const BUFFER_COUNT: usize = 3;

const AUDIO_QUEUE_RUN_STATE_STOPPED: u32 = 0;

const MAX_VOLUME: usize = 16;
const VOLUME_STEP: usize = 1;

pub struct PlaybackContext {
    playback_file: AudioFileID,
    format: AudioStreamBasicDescription,
    buffer_size: u32,
    is_vbr: bool,
    packets_per_buffer: PacketCount,
}

impl PlaybackContext {
    pub fn new(path: &str) -> PlaybackResult<Self> {
        let path = cstring_path(path)?;
        let audio_file = audio_file_open(&path)?;

        // Use
        //  - the theoretical max size of a packet of this format
        //  - some heuristics
        //  - a desired buffer duration (aproximate)
        // to determine
        // - how big each buffer needs to be
        // - how many packet to read each time we fill a buffer

        let format = audio_file_read_basic_description(audio_file)?;
        let max_packet_size = audio_file_read_packet_size_upper_bound(audio_file)?;

        let buffer_size = if format.frames_per_packet != 0 {
            // If frames per packet are known, tailor the buffer size.
            let frames = format.sample_rate * BUFFER_SECONDS_HINT;
            let packets = (frames / (format.frames_per_packet as f64)).ceil() as u32;
            let size = packets * max_packet_size;
            let size = cmp::max(size, LOWER_BUFFER_SIZE_HINT);
            cmp::min(size, UPPER_BUFFER_SIZE_HINT)
        } else {
            // If frames per packet is not known, fallback to something large enough
            cmp::max(max_packet_size, UPPER_BUFFER_SIZE_HINT)
        };

        let is_vbr = format.bytes_per_packet == 0 || format.frames_per_packet == 0;
        let packets_per_buffer = buffer_size / max_packet_size;

        Ok(PlaybackContext {
            playback_file: audio_file,
            packets_per_buffer,
            format,
            buffer_size,
            is_vbr,
        })
    }

    pub fn file_metadata(&self) -> PlaybackResult<Vec<(String, String)>> {
        audio_file_read_metadata(self.playback_file).map_err(|e| e.into())
    }

    pub fn estimated_duration(&self) -> PlaybackResult<f64> {
        audio_file_read_estimated_duration(self.playback_file).map_err(|e| e.into())
    }

    pub fn new_audio_callback_handler(&self, notifier: CallbackNotifier) -> AudioCallbackHandler {
        AudioCallbackHandler {
            playback_file: self.playback_file,
            is_vbr: self.is_vbr,
            notifier,
            packets_per_buffer: self.packets_per_buffer,
            current_packet: 0,
            finished: false,
        }
    }

    pub fn new_audio_player(
        &self,
        handler: &mut AudioCallbackHandler,
    ) -> PlaybackResult<AudioFilePlayer> {
        let handler_ptr = handler as *mut _ as *mut c_void;
        let output_queue = output_queue_create(&self.format, handler_ptr)?;

        let packet_descriptions = match self.is_vbr {
            true => self.packets_per_buffer,
            false => 0,
        };

        let buffers = create_buffers(output_queue, packet_descriptions, self.buffer_size)?;

        if let Some(cookie) = audio_file_read_magic_cookie(self.playback_file)? {
            audio_queue_set_magic_cookie(output_queue, cookie)?
        }

        audio_queue_listen_to_run_state(output_queue, handler_ptr)?;

        // TODO: Do this on start?
        // While handle_buffer is usually invoked from the output queues internal
        // callback thread to refill a buffer, we call it a few times before
        // starting to pre load the buffers with audio. This means that any
        // error during pre-buffering is not directly surfaced here, but
        // reported back via the handler as an error event.
        // For small files, some buffers might remain unused.
        for buffer_ref in buffers {
            handle_buffer(handler_ptr, output_queue, buffer_ref);
        }

        let default_meter = AudioQueueLevelMeterState::default();
        let meter_count = self.format.channels_per_frame as usize;
        let meters = vec![default_meter; meter_count];

        Ok(AudioFilePlayer {
            output_queue,
            handler: PhantomData,
            sample_rate: self.format.sample_rate,
            meter_state: meters.into_boxed_slice(),
        })
    }
}

impl Drop for PlaybackContext {
    fn drop(&mut self) {
        audio_file_close(self.playback_file).expect("Failed to close audio file");
    }
}

pub struct AudioCallbackHandler {
    playback_file: AudioFileID,
    is_vbr: bool,
    notifier: CallbackNotifier,
    packets_per_buffer: PacketCount,
    current_packet: PacketPosition,
    finished: bool,
}

impl AudioCallbackHandler {
    fn handle_buffer(&mut self, audio_queue: AudioQueueRef, buffer: AudioQueueBufferRef) {
        if self.finished {
            return;
        }

        let read_result = audio_file_read_packet_data(
            self.playback_file,
            self.current_packet,
            self.packets_per_buffer,
            buffer,
            self.is_vbr,
        );

        let packets_read = match read_result {
            //TODO: Report error properly
            Ok(packets_read) => packets_read,
            Err(error) => {
                //TODO: Report error properly
                self.finished = true;
                return;
            }
        };

        if packets_read == 0 {
            self.finished = true;
            // Request an asynchronous stop so that buffered audio can finish playing.
            // Queue stopping is detected via seperate callback to property listener.
            //TODO: Handle and report Error
            audio_queue_stop(audio_queue, false).expect("oh no");
            return;
        }

        match audio_queue_enqueue_buffer(audio_queue, buffer) {
            Ok(()) => {
                self.current_packet += packets_read as i64;
            }
            // Attempting to enqueue during reset can be expected when the user
            // has stopped the queue before playback has finished.
            Err(SystemErrorCode(audio_toolbox::AUDIO_QUEUE_ERROR_ENQUEUE_DURING_RESET)) => {
                self.finished = true;
            }
            // Anything else is probably a legitimate error condition
            Err(SystemErrorCode(code)) => {
                //TODO: Report error
                self.finished = true;
            }
        }
    }

    fn handle_running_state_change(&mut self, audio_queue: AudioQueueRef) {
        match audio_queue_read_run_state(audio_queue) {
            Ok(AUDIO_QUEUE_RUN_STATE_STOPPED) => {
                self.notifier
                    .trigger_playback_finished_event()
                    .expect("failed to trigger playback event");
            }
            Ok(_) => {
                // Any other value is the queue starting
                self.notifier
                    .trigger_playback_started_event()
                    .expect("failed to trigger playback event");
            }
            //TODO: Feed back error to controller?
            Err(error) => panic!("booooo!: {error}\r\n"),
        }
    }
}

// SAFETY: Although not immediately apparent from the fields in the
// AudioFilePlayer struct, the output queue will internally hold a raw pointer
// to `handler`. The output queue will use this pointer to mutate the handler as
// it advances through the audio file a buffer at a time. Therefore the
// PhantomData marker is used to enforce ownership of the handler for the
// lifetime of the AudioFilePlayer. After which the output queue will have been
// disposed of and handler _should_ be safe to access again.
pub struct AudioFilePlayer<'a> {
    output_queue: AudioQueueRef,
    handler: PhantomData<&'a mut AudioCallbackHandler>,
    sample_rate: f64,
    //TODO: Assert somehow that this is at least > 1
    meter_state: Box<[AudioQueueLevelMeterState]>,
}

impl<'a> AudioFilePlayer<'a> {
    pub fn start_playback(&mut self) -> PlaybackResult<()> {
        audio_queue_enable_metering(self.output_queue)?;
        audio_queue_start(self.output_queue)?;
        Ok(())
    }

    pub fn pause(&mut self) -> PlaybackResult<()> {
        audio_queue_pause(self.output_queue)?;
        Ok(())
    }

    pub fn resume(&mut self) -> PlaybackResult<()> {
        audio_queue_start(self.output_queue)?;
        Ok(())
    }

    pub fn stop(&mut self) -> PlaybackResult<()> {
        // Stop the queue synchronously
        audio_queue_stop(self.output_queue, true)?;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: &PlaybackVolume) -> PlaybackResult<()> {
        let gain = volume.gain();
        assert!(gain >= 0.0f32);
        assert!(gain <= 1.0f32);
        audio_queue_set_volume(self.output_queue, gain)?;
        Ok(())
    }

    pub fn get_meter_level(&mut self) -> PlaybackResult<[f32; 2]> {
        audio_queue_read_meter_level(self.output_queue, &mut self.meter_state)?;

        // TODO: Panic (or maybe err?) if meter_state.len() is 0
        let mut levels = self.meter_state.iter().map(|chan| chan.average_power);
        Ok(match (levels.next(), levels.next()) {
            (Some(chan_1), None) => [chan_1, chan_1],
            (Some(chan_1), Some(chan_2)) => [chan_1, chan_2],
            _ => [0.0, 0.0], // This shouldn't happen
        })
    }

    pub fn get_playback_time(&mut self) -> PlaybackResult<Option<f64>> {
        let time = audio_queue_read_current_sample_time(self.output_queue)?;
        let time = time.map(|t| t / self.sample_rate);
        Ok(time)
    }
}

impl<'a> Drop for AudioFilePlayer<'a> {
    fn drop(&mut self) {
        // Dispose of the queue synchronously
        audio_queue_dispose(self.output_queue, true).expect("Failed to dispose of audio queue");
    }
}

pub struct PlaybackVolume {
    volume: usize,
}

impl PlaybackVolume {
    pub fn new() -> Self {
        PlaybackVolume { volume: MAX_VOLUME }
    }

    pub fn increment(&mut self) {
        self.volume = cmp::min(self.volume + VOLUME_STEP, MAX_VOLUME);
    }

    pub fn decrement(&mut self) {
        self.volume = cmp::max(self.volume.saturating_sub(VOLUME_STEP), 0);
    }

    pub fn gain(&self) -> f32 {
        self.volume as f32 / MAX_VOLUME as f32
    }
}

// TODO: Should always we ask for more packets than buffer can hold to ensure
// the buffer gets fully used?
//
// We could calculate the max packets per buffer instead of minimum? I.e
// optimistic instead of pessamistic.
//
// This would possibly take advantage of the properties AudioFileReadPacketData
// has over AudioFileReadPackets?
//
// We would still need an upper limit (and overallocate) the
// packet_descriptions.
//
// Is there somehow we could test this by detecting underutilized buffers?
// The queue could continue to callback with remaining buffers.
// Avoid unnecessary attempts to read the file again.

// This handler assumes `buffer` adheres to several invarients:
// - Is at least `packets_per_buffer` big
// - Was allocated with packet descriptions (if needed)
// - Belong to `audio_queue`
extern "C" fn handle_buffer(
    user_data: *mut c_void,
    audio_queue: AudioQueueRef,
    buffer: AudioQueueBufferRef,
) {
    unsafe {
        let handler = &mut *(user_data as *mut AudioCallbackHandler);
        handler.handle_buffer(audio_queue, buffer);
    }
}

extern "C" fn handle_running_state_change(
    user_data: *mut c_void,
    audio_queue: AudioQueueRef,
    property: AudioQueuePropertyID,
) {
    // The handler should only react to changes to the "is running" property
    assert!(property == audio_toolbox::AUDIO_QUEUE_PROPERTY_IS_RUNNING);

    unsafe {
        let handler = &mut *(user_data as *mut AudioCallbackHandler);
        handler.handle_running_state_change(audio_queue);
    }
}

fn create_buffers(
    output_queue: AudioQueueRef,
    packet_descriptions: u32,
    buffer_size: u32,
) -> SystemResult<Vec<AudioQueueBufferRef>> {
    unsafe {
        vec![MaybeUninit::uninit(); BUFFER_COUNT]
            .into_iter()
            //TODO: Can we allocate buffers _without_ packet descriptions if we dont need them?
            .map(|mut buffer_ref| {
                let status = audio_toolbox::audio_queue_allocate_buffer_with_packet_descriptions(
                    output_queue,
                    buffer_size,
                    packet_descriptions,
                    buffer_ref.as_mut_ptr(),
                );

                if status == 0 {
                    Ok(buffer_ref.assume_init())
                } else {
                    Err(SystemErrorCode(status))
                }
            })
            .collect()
    }
}

fn cstring_path(path: &str) -> Result<CString, PathError> {
    if path.is_empty() {
        return Err(PathError::PathIsEmpty);
    }

    Ok(CString::new(path)?)
}

fn audio_file_read_packet_data(
    file: AudioFileID,
    from_packet: PacketPosition,
    packets: PacketCount,
    buffer: AudioQueueBufferRef,
    is_vbr: bool,
) -> SystemResult<PacketCount> {
    unsafe {
        let buffer = &mut *buffer;
        let mut num_bytes = buffer.audio_data_bytes_capacity;
        let mut num_packets = packets;

        let packet_descs_ptr = if is_vbr {
            buffer.packet_descriptions
        } else {
            ptr::null_mut()
        };

        let status = audio_toolbox::audio_file_read_packet_data(
            file,
            false, // dont use caching
            &mut num_bytes,
            packet_descs_ptr,
            from_packet,
            &mut num_packets,
            buffer.audio_data,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        buffer.audio_data_byte_size = num_bytes;
        buffer.packet_description_count = if is_vbr { num_packets } else { 0 };

        Ok(num_packets)
    }
}

fn audio_file_open(path: &CStr) -> SystemResult<AudioFileID> {
    let path = path.to_bytes();

    unsafe {
        // Create URL
        let url_ref = core_foundation::cfurl_create_from_filesystem_representation(
            ptr::null(), // Use default allocator
            path.as_ptr(),
            path.len() as isize,
            false, // Not a directory
        );

        // Open file
        let mut file_id = MaybeUninit::uninit();
        let status = audio_toolbox::audio_file_open_url(
            url_ref,
            audio_toolbox::AUDIO_FILE_READ_PERMISSION,
            0, // No file hints
            file_id.as_mut_ptr(),
        );

        // Dont need the CFURL anymore
        core_foundation::cf_release(url_ref as *const c_void);

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        let file_id = file_id.assume_init();
        Ok(file_id)
    }
}

fn audio_file_close(file: AudioFileID) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_file_close(file);
        if status != 0 {
            return Err(SystemErrorCode(status));
        }
    }
    Ok(())
}

fn audio_file_read_metadata(file: AudioFileID) -> SystemResult<Vec<(String, String)>> {
    unsafe {
        let info_dict =
            audio_file_get_property(file, audio_toolbox::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

        // Extract keys and values
        let count = core_foundation::cfdictionary_get_count(info_dict);
        let mut keys = vec![0 as core_foundation::CFStringRef; count as usize];
        let mut values = vec![0 as core_foundation::CFStringRef; count as usize];

        core_foundation::cfdictionary_get_keys_and_values(
            info_dict,
            keys.as_mut_ptr() as *mut *const c_void,
            values.as_mut_ptr() as *mut *const c_void,
        );

        // Filter out non CFString values and convert to Rust strings
        // Note: We eagerly collect to force conversation before the dictionary is
        // released

        let cfstring_type_id = core_foundation::cfstring_get_type_id();

        let properties = keys
            .into_iter()
            .zip(values.into_iter())
            .filter(|(_, v)| {
                core_foundation::cf_get_type_id(*v as *const c_void) == cfstring_type_id
            })
            .map(|(k, v)| (cfstring_to_string(k), cfstring_to_string(v)))
            .collect();

        core_foundation::cf_release(info_dict as *const c_void);

        Ok(properties)
    }
}

fn audio_file_read_basic_description(
    file: AudioFileID,
) -> SystemResult<AudioStreamBasicDescription> {
    audio_file_get_property(file, audio_toolbox::AUDIO_FILE_PROPERTY_DATA_FORMAT)
}

fn audio_file_read_packet_size_upper_bound(file: AudioFileID) -> SystemResult<u32> {
    audio_file_get_property(
        file,
        audio_toolbox::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND,
    )
}

fn audio_file_read_estimated_duration(file: AudioFileID) -> SystemResult<f64> {
    audio_file_get_property(file, audio_toolbox::AUDIO_FILE_PROPERTY_ESTIMATED_DURATION)
}

// This only works with sized types
fn audio_file_get_property<T>(
    file_id: audio_toolbox::AudioFileID,
    property: audio_toolbox::AudioFilePropertyID,
) -> SystemResult<T> {
    unsafe {
        let mut data = MaybeUninit::<T>::uninit();
        let mut data_size = mem::size_of::<T>() as u32;

        let status = audio_toolbox::audio_file_get_property(
            file_id,
            property,
            &mut data_size as *mut _,
            data.as_mut_ptr() as *mut c_void,
        );
        let data = data.assume_init();

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // audio_file_get_property outputs the number of bytes written to data_size
        // Check to see if this is correct for the given type
        assert!(data_size == mem::size_of::<T>() as u32);

        Ok(data)
    }
}

fn audio_file_read_magic_cookie(file: AudioFileID) -> SystemResult<Option<Vec<u8>>> {
    unsafe {
        // Check to see if there is a cookie, and if so how large it is.
        let mut cookie_size: u32 = 0;
        let mut is_writable: u32 = 0;
        let status = audio_toolbox::audio_file_get_property_info(
            file,
            audio_toolbox::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
            &mut cookie_size as *mut _,
            &mut is_writable as *mut _,
        );

        // No magic cookie data
        if status == audio_toolbox::AUDIO_FILE_ERROR_UNSUPPORTED_PROPERTY || cookie_size == 0 {
            return Ok(None);
        }

        // Some other status is probably an error
        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // Read the cookie
        let mut cookie_data: Vec<u8> = vec![0; cookie_size as usize];
        let mut data_size = cookie_size;

        let status = audio_toolbox::audio_file_get_property(
            file,
            audio_toolbox::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
            &mut data_size as *mut _,
            cookie_data.as_mut_ptr() as *mut c_void,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        assert!(data_size == cookie_size);
        Ok(Some(cookie_data))
    }
}

fn output_queue_create(
    format: *const AudioStreamBasicDescription,
    user_data: *mut c_void,
) -> SystemResult<AudioQueueRef> {
    unsafe {
        let mut output_queue = MaybeUninit::uninit();
        let status = audio_toolbox::audio_queue_new_output(
            format,
            handle_buffer,
            user_data,
            std::ptr::null(), // Run loop
            std::ptr::null(), // Run loop mode
            0,                // flags
            output_queue.as_mut_ptr(),
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }
        let output_queue = output_queue.assume_init();
        Ok(output_queue)
    }
}

fn audio_queue_set_magic_cookie(queue: AudioQueueRef, cookie: Vec<u8>) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_set_property(
            queue,
            audio_toolbox::AUDIO_QUEUE_PROPERTY_MAGIC_COOKIE_DATA,
            cookie.as_ptr() as *const c_void,
            cookie.len() as u32,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_start(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_start(queue, ptr::null());

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_stop(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_stop(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_dispose(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_dispose(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_pause(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_pause(queue);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_enqueue_buffer(
    queue: AudioQueueRef,
    buffer: AudioQueueBufferRef,
) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_enqueue_buffer(
            queue,
            buffer,
            // Packet descriptions are supplied via buffer itself
            0,
            ptr::null(),
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_listen_to_run_state(
    queue: AudioQueueRef,
    user_data: *mut c_void,
) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_add_property_listener(
            queue,
            audio_toolbox::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
            handle_running_state_change,
            user_data,
        );
        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_read_run_state(queue: AudioQueueRef) -> SystemResult<u32> {
    unsafe {
        let mut data = MaybeUninit::<u32>::uninit();
        let mut data_size = mem::size_of::<u32>() as u32;

        let status = audio_toolbox::audio_queue_get_property(
            queue,
            audio_toolbox::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
            data.as_mut_ptr() as *mut c_void,
            &mut data_size as *mut _,
        );

        let data = data.assume_init();

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // audio_queue_get_property outputs the number of bytes written to data_size
        // Check to see if this is correct
        assert!(data_size == mem::size_of::<u32>() as u32);

        Ok(data)
    }
}

fn audio_queue_set_volume(queue: AudioQueueRef, gain: f32) -> SystemResult<()> {
    unsafe {
        let status = audio_toolbox::audio_queue_set_parameter(
            queue,
            audio_toolbox::AUDIO_QUEUE_PARAMETER_VOLUME,
            gain,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_enable_metering(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let enabled: u32 = 1;

        let status = audio_toolbox::audio_queue_set_property(
            queue,
            audio_toolbox::AUDIO_QUEUE_PROPERTY_ENABLE_LEVEL_METERING,
            &enabled as *const _ as *const c_void,
            mem::size_of::<u32>() as u32,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_read_meter_level(
    queue: AudioQueueRef,
    meter_state: &mut Box<[AudioQueueLevelMeterState]>,
) -> SystemResult<()> {
    unsafe {
        let meter_size = mem::size_of::<AudioQueueLevelMeterState>();
        let expected_size = (meter_size * meter_state.len()) as u32;
        let mut data_size = expected_size;

        let status = audio_toolbox::audio_queue_get_property(
            queue,
            audio_toolbox::AUDIO_QUEUE_PROPERTY_LEVEL_METER_STATE,
            meter_state.as_mut_ptr() as *mut c_void,
            &mut data_size as *mut _,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        assert!(data_size == expected_size);
        Ok(())
    }
}

fn audio_queue_read_current_sample_time(queue: AudioQueueRef) -> SystemResult<Option<f64>> {
    //TODO: Check this isnt problematically large to repetedly zero
    let mut timestamp = AudioTimeStamp::default();
    unsafe {
        let status =
            audio_queue_get_current_time(queue, ptr::null(), &mut timestamp, ptr::null_mut());

        // If the queue isn't running, it has no playback time
        if status == audio_toolbox::AUDIO_QUEUE_ERROR_INVALID_RUN_STATE {
            return Ok(None);
        }
        if status != 0 {
            return Err(SystemErrorCode(status));
        }
    }
    Ok(Some(timestamp.sample_time))
}

unsafe fn cfstring_to_string(cfstring: core_foundation::CFStringRef) -> String {
    assert!(!cfstring.is_null());

    let string_len = core_foundation::cfstring_get_length(cfstring);

    // This is effectively asking how big a buffer we are going to need
    let mut bytes_required = 0;
    core_foundation::cfstring_get_bytes(
        cfstring,
        core_foundation::CFRange {
            location: 0,
            length: string_len,
        },
        core_foundation::CFSTRING_ENCODING_UTF8,
        0,               // no loss byte
        false,           // no byte order marker
        ptr::null_mut(), // dont actually capture any bytes
        0,               // buffer size of 0 as no buffer supplied
        &mut bytes_required,
    );

    // Now actually copy out the bytes
    let mut buffer = vec![b'\x00'; bytes_required as usize];
    let mut bytes_written = 0;

    let chars_converted = core_foundation::cfstring_get_bytes(
        cfstring,
        core_foundation::CFRange {
            location: 0,
            length: string_len,
        },
        core_foundation::CFSTRING_ENCODING_UTF8,
        0,     // no loss byte
        false, // no byte order marker
        buffer.as_mut_ptr(),
        buffer.len() as core_foundation::CFIndex,
        &mut bytes_written,
    );

    assert!(chars_converted == string_len);
    assert!(bytes_written as usize == buffer.len());

    String::from_utf8_unchecked(buffer)
}
