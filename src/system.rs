//! Selected FFI bindings to AudioToolbox and related frameworks.
//!
//! To facilitate cross referencing with macOS API documentation,
//! types that cross the FFI boundary generally follow simmilar
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

/// Determines if an audio file should be readable, writeable or both.
pub type AudioFilePermissions = i8;

/// Used to indicate that an audio file should be read only.
pub const AUDIO_FILE_READ_PERMISSION: i8 = 1;

/// A refeferance to an opaque type representing an audio queue object.
///
/// An audio queue enables recording and playback of audio in macOS.
///
/// It does the work of:
/// - Connecting to audio hardware
/// - Managing memory
/// - Employing codecs, as needed, for compressed audio formats
/// - Mediating recording or playback
pub type AudioQueueRef = *const OpaqueAudioQueue;

/// Speficies the format of an audio stream.
///
/// An audio stream is a continious sequence of numeric samples, arranged into
/// one or more discrete channels of monophonic sound. Samples that are
/// co-incident in time are refered to as a "frame". E.g a stereo sound file has
/// two samples per frame.
///
/// For a given audio format, the smallest meaningful collection of contigious
/// frames is known as a "packet". While for linear PCM audio, a packet contains
/// a single frame, in compressed formats a packet typcially holds more, or can
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
/// applicable to the format. Always initialize the fields of a new audio stream
/// basic description structure to zero, as shown here:
/// AudioStreamBasicDescription myAudioDataFormat = {0};
#[repr(C)]
pub struct AudioStreamBasicDescription {
    /// Number of frames per second of uncompressed (or decompressed) audio
    sample_rate: f64,
    /// General kind of data in the stream
    format_id: u32,
    /// Flags for the format indicated by format_id
    format_flags: u32,
    /// Number of bytes in each packet
    bytes_per_packet: u32,
    /// Number of sample frames in each packet
    frames_per_packet: u32,
    /// Number of bytes in a sample frame
    bytes_per_frame: u32,
    /// Number of channels in each frame of data
    channels_per_frame: u32,
    /// Number of bits of sample data for each channel in a
    bits_per_channel: u32,
    /// Pads out the structure to force an even 8 byte alignment
    reserved: u32,
}

/// A reference to an audio queue buffer
pub type AudioQueueBufferRef = *mut AudioQueueBuffer;

//TODO: Implement, doc
#[repr(C)]
pub struct AudioQueueBuffer {}

/// This type defines a callback function that is called each time its
/// associated output audio queue has finished processing a buffer of data, and
/// is ready for the buffer to be reused. Typcially a implementation of this
/// callback will immediately refill and re-enque the buffer.
///
/// The `in_aq` parameter specifies which audio queue invoked the callback,
/// and the `in_buffer` parameter will point to the newly available buffer.
///
/// A callback is associated with an audio queue when the audio queue is
/// created. This is also the point at which custom user data is defined. User
/// data is made available in the callback via the `in_user_data` paramater and
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

    pub type OpaqueAudioFileID;
    pub type OpaqueAudioQueue;
    /// Open an audio file with the AudioToolbox framework.
    ///
    /// Opens the audio file specified by `in_ref_file`.
    ///
    /// The `in_permissions` parameter determines if the file is opened as read,
    /// write or read and write.
    ///
    /// If the name of the file has no extenion and the type of the file can't
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
    /// relative path components. True if the pathname indicates a directory,
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
