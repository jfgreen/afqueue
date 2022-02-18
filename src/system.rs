//! Selected FFI bindings to AudioToolbox and related frameworks.
//!
//! To facilitate cross referencing with macOS API documentation,
//! types that cross the FFI boundary generally follow similar
//! naming and type aliasing conventions to those found in the macOS SDK header
//! files.

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr;

/// A type free reference to an opaque Core Foundation object.
///
/// This type is accepted by polymorphic functions like `cf_release`.
pub type CFTypeRef = *const c_void;

/// A reference to a CFAllocator object.
///
/// CFAllocatorRef is used in many Core Foundation parameters which need to
/// allocate memory. For our use case, we can supply an null pointer to tell
/// Core Foundation to use the default allocator.
pub type CFAllocatorRef = *const c_void;

/// macOS error code.
pub type OSStatus = i32;

/// A reference to an opaque type representing an audio file object.
pub type AudioFileID = *const OpaqueAudioFileID;

/// Constant value used to supply audio file type hints.
pub type AudioFileTypeID = u32;

/// Constant value identifying an audio file property.
pub type AudioFilePropertyID = u32;

/// Determines if an audio file should be readable, writable or both.
pub type AudioFilePermissions = i8;

/// Used to indicate that an audio file should be read only.
pub const AUDIO_FILE_READ_PERMISSION: i8 = 1;

/// Audio file property constant used to access information about a file.
///
/// This constant can be used with `audio_file_get_property` to obtain a Core
/// Foundation dictionary containin information describing an audio file.
pub const AUDIO_FILE_PROPERTY_INFO_DICTIONARY: u32 = u32::from_be_bytes(*b"info");

/// A reference to an opaque type representing an audio queue object.
///
/// An audio queue enables recording and playback of audio in macOS.
///
/// It does the work of:
/// - Connecting to audio hardware
/// - Managing memory
/// - Employing codecs, as needed, for compressed audio formats
/// - Mediating recording or playback
pub type AudioQueueRef = *const OpaqueAudioQueue;

/// Specifies the format of an audio stream.
///
/// An audio stream is a continuous sequence of numeric samples, arranged into
/// one or more discrete channels of monophonic sound. Samples that are
/// co-incident in time are referred to as a "frame". E.g a stereo sound file
/// has two samples per frame.
///
/// For a given audio format, the smallest meaningful collection of contiguous
/// frames is known as a "packet". While for linear PCM audio, a packet contains
/// a single frame, in compressed formats a packet typically holds more, or can
/// even have a variable size.
///
/// To determine the duration represented by one packet, use the `sample_rate`
/// field with the `frames_per_packet`  field as follows:
///
/// The duration represented by a single packet can be calculated as follows:
/// ```
/// duration = (1 / sample_rate) * frames_per_packet
/// ```
///
/// A range of audio formats can be described by `AudioStreamBasicDescription`.
/// However, for formats that have channels have unequal sizes, (VBR, and some
/// CBR formats), `AudioStreamPacketDescription` is also needed to describe each
/// individual packet.
///
/// A field value of 0 indicates that the value is either unknown or not
/// applicable to the format. Always initialise the fields of a new audio stream
/// basic description structure to zero, as shown here:
/// AudioStreamBasicDescription myAudioDataFormat = {0};
#[repr(C)]
pub struct AudioStreamBasicDescription {
    /// Number of frames per second of uncompressed (or decompressed) audio.
    sample_rate: f64,
    /// General kind of data in the stream.
    format_id: u32,
    /// Flags for the format indicated by format_id.
    format_flags: u32,
    /// Number of bytes in each packet.
    bytes_per_packet: u32,
    /// Number of sample frames in each packet.
    frames_per_packet: u32,
    /// Number of bytes in a sample frame.
    bytes_per_frame: u32,
    /// Number of channels in each frame of data.
    channels_per_frame: u32,
    /// Number of bits of sample data for each channel.
    bits_per_channel: u32,
    /// Pads out the structure to force an even 8 byte alignment
    reserved: u32,
}

//TODO: Implement, doc
#[repr(C)]
pub struct AudioStreamPacketDescription {}

/// A reference to an audio queue buffer.
pub type AudioQueueBufferRef = *mut AudioQueueBuffer;

/// A buffer of audio data associated with an audio queue.
/// Each audio queue manages a set of these buffers.
///
/// The buffer size, indicated by `audio_data_bytes_capacity` is set when the
/// buffer is allocated, and can not be changed.
///
/// When providing buffers to an output audio queue for playback, you must set
/// `packet_description_count` and `audio_data_byte_size`. Conversely, when
/// receiving buffers from a recording queue, these values will be instead set
/// by the audio queue.
///
/// Note: While it's possible to write to the data pointed to by the
/// `audio_data` field the pointer address itself must not be changed.
#[repr(C)]
pub struct AudioQueueBuffer {
    /// The size of the audio queue buffer, in bytes.
    audio_data_bytes_capacity: u32,
    /// Pointer to audio data.
    audio_data: *const c_void,
    /// The number of bytes of audio data in the `audio_data` field.
    audio_data_byte_size: u32,
    /// Custom data specified during audio queue creation.
    user_data: *const c_void,
    /// The max number of entries that can be stored in `packet_descriptions`.
    packet_description_capacity: u32,
    /// Pointer to an array of descriptions (when packet size varies).
    packet_descriptions: *const AudioStreamPacketDescription,
    /// The number of packet descriptions in the buffer.
    packet_description_count: u32,
}

/// Callback to respond when an output audio queue has a buffer to reuse.
///
/// This type defines a callback function that is called each time its
/// associated output audio queue has finished processing a buffer of data, and
/// is ready for the buffer to be reused. Typically a implementation of this
/// callback will immediately refill and re-enqueue the buffer.
///
/// The `in_aq` parameter specifies which audio queue invoked the callback,
/// and the `in_buffer` parameter will point to the newly available buffer.
///
/// A callback is associated with an audio queue when the audio queue is
/// created. This is also the point at which custom user data is defined. User
/// data is made available in the callback via the `in_user_data` parameter and
/// will typically contain information about the currently data format and queue
/// state.
///
/// Note: When this callback is invoked, you can not assume that the sound data
/// already in the buffer has been played.
pub type AudioQueueOutputCallback = unsafe extern "C" fn(
    in_user_data: *mut c_void,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
);

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {

    /// An opaque data type that represents an audio file.
    pub type OpaqueAudioFileID;
    /// An opaque data type that represents an audio queue.
    pub type OpaqueAudioQueue;

    /// Open an audio file with the AudioToolbox framework.
    ///
    /// Opens the audio file specified by `in_ref_file`.
    ///
    /// The `in_permissions` parameter determines if the file is opened as read,
    /// write or read and write.
    ///
    /// If the name of the file has no extension and the type of the file can't
    /// be easily or uniquely determined from its contents, then `in_file_hint`
    /// can be used to indicate the file type. Set `in_file_hint` to zero to
    /// omit a hint.
    ///
    /// Upon success the audio file id pointed to by `out_audio_file` will be
    /// set to the ID of the open file.
    ///
    /// If opening the audio file fails, `audio_file_open_url` will return an
    /// error.
    #[link_name = "AudioFileOpenURL"]
    pub fn audio_file_open_url(
        in_file_ref: CFURLRef,
        in_permissions: AudioFilePermissions,
        in_file_type_hint: AudioFileTypeID,
        out_audio_file: *mut AudioFileID,
    ) -> OSStatus;

    /// Close an audio file
    #[link_name = "AudioFileClose"]
    pub fn audio_file_close(in_audio_file: AudioFileID) -> OSStatus;

    /// Retrieve information about an audio file property.
    ///
    /// For a particular property of an audio file (specified by
    /// `in_property_id` and `in_audio_file` respectively), fetch the
    /// following:
    /// - The size in bytes of the parameter, written to `out_data_size`.
    /// - Whether or not the property is writable, written to `is_writable`.
    ///   This value will equal 1 if true and zero if false.
    ///
    /// Returns an error if unsuccessful.
    #[link_name = "AudioFileGetPropertyInfo"]
    pub fn audio_file_get_property_info(
        in_audio_file: AudioFileID,
        in_property_id: AudioFilePropertyID,
        out_data_size: *mut u32,
        is_writable: *mut u32,
    ) -> OSStatus;

    /// Get the value of an audio file property by copying it into a buffer.
    ///
    /// For the audio file indicated by the `in_audio_file`, fetches the
    /// property specified by `in_property_id`, and writes it to
    /// `out_property_data`.
    ///
    /// The `io_data_size` parameter serves two purposes.
    /// When calling this function, it should contain the size of the
    /// buffer supplied to `out_property_data`. On function return, its
    /// value will contain the number of bytes written to the buffer.
    ///
    /// To help correctly size the output buffer, the
    /// `audio_file_get_property_info` function can be used to determine the
    /// size of the property ahead of time.
    ///
    /// Some audio file property values are C types and others are Core
    /// Foundation objects. If this function returns a Core Foundation
    /// object, then you are responsible for releasing it.
    ///
    /// Returns an error if unsuccessful.
    #[link_name = "AudioFileGetProperty"]
    pub fn audio_file_get_property(
        in_audio_file: AudioFileID,
        in_property_id: AudioFilePropertyID,
        io_data_size: *mut u32,
        out_property_data: *mut c_void,
    ) -> OSStatus;

    //TODO: Doc
    /*
        #[link_name = "AudioQueueNewOutput"]
        fn audio_queue_new_output(
            in_format: *const AudioStreamBasicDescription,
            in_callback_proc: AudioQueueOutputCallback,
            in_user_data: *const c_void,
            in_callback_run_loop: CFRunLoopRef,
            in_callback_run_loop_mode: CFStringRef,
            in_flags: u32,
            out_aq: *mut AudioQueueRef,
        ) -> OSStatus;
    */
}

/// A reference to an opaque CFURL object.
pub type CFURLRef = *const CFURL;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {

    /// Core Foundation URL, provides functionality to manipulate URL strings.
    pub type CFURL;

    /// Creates a new Core Foundation URL (CFURL) from the systems "native"
    /// string representation.
    ///
    /// Core Foundation describes the "native" representation as being the
    /// format used in POSIX function calls. In our case this is the same as
    /// the underlying buffer of bytes in a Rust CString.
    ///
    /// The string is by passed into the function via the `buffer` parameter, a
    /// raw pointer pointing to start of the string. Therefore the caller
    /// must also supply `buf_len`, indicating the length of the
    /// string (not including null termination).
    ///
    /// It's the responsibility of the caller to ensure that the
    /// `buffer` parameter points to start of a valid sequence of character
    /// bytes, and that `buf_len` correctly describes the length of this
    /// sequence.
    ///
    /// Creating a `CFURL` requires allocating memory.
    /// To specify which Core Foundation allocator to use for this,
    /// pass in an CFAllocatorRef via `allocator`. Pass a null pointer or
    /// kCFAllocatorDefault to use the current default allocator.
    ///
    /// The boolean `is_directory` specifies whether
    /// the string is treated as a directory path when resolving against
    /// relative path components. True if the path name indicates a directory,
    /// false otherwise.
    ///
    /// The return value of this function is a pointer to an opaque CFURL
    /// object. This can be passed into other functions requiring a
    /// reference to a CFURL.
    #[link_name = "CFURLCreateFromFileSystemRepresentation"]
    pub fn cfurl_create_from_filesystem_representation(
        allocator: CFAllocatorRef,
        buffer: *const u8,
        buf_len: isize,
        is_directory: bool,
    ) -> CFURLRef;

    /// Release a claim to a Core Foundation object.
    ///
    /// This will decrease its reference count by one. If its new reference
    /// count is zero then the object will be destroyed and its memory
    /// deallocated.
    ///
    /// When a Core Foundation object is created its reference count is set to
    /// initially set to one.
    #[link_name = "CFRelease"]
    pub fn cf_release(cf: CFTypeRef);
}
