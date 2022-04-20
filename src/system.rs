//! Selected FFI bindings to AudioToolbox and related system frameworks.
//!
//! To facilitate cross referencing with macOS API documentation,
//! types that cross the FFI boundary generally follow similar
//! naming and type aliasing conventions to those found in the macOS SDK header
//! files.

use std::ffi::c_void;

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

/// Constant used to interact with an audio files metadata.
///
/// This constant can be used with `audio_file_get_property` to obtain a Core
/// Foundation dictionary containing information describing an audio file.
///
/// The caller is responsable for releasing the dictionary via `cf_release`.
pub const AUDIO_FILE_PROPERTY_INFO_DICTIONARY: u32 = u32::from_be_bytes(*b"info");

/// Constant used to interact with an audio files format description.
///
/// Using this constant with `audio_file_get_property` will return an
/// AudioStreamBasicDescription describing the files audio format.
pub const AUDIO_FILE_PROPERTY_DATA_FORMAT: u32 = u32::from_be_bytes(*b"dfmt");

/// Constant used to interact with an audio files cookie data.
///
/// Magic cookie data encodes format specific data
pub const AUDIO_FILE_PROPERT_MAGIC_COOKIE_DATA: u32 = u32::from_be_bytes(*b"mgic");

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

/// Specifies an audio format
pub type AudioFormatID = u32;

/// Specifies format specific flags
pub type AudioFormatFlags = u32;

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
#[derive(Debug)]
#[repr(C)]
pub struct AudioStreamBasicDescription {
    /// Number of frames per second of uncompressed (or decompressed) audio.
    sample_rate: f64,
    /// General kind of data in the stream.
    format_id: AudioFormatID,
    /// Flags for the format indicated by format_id.
    format_flags: AudioFormatFlags,
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

/// Supplementary information used to describe variable sized audio packets.
///
/// Describes a packet in a buffer of data where the size of each packet may
/// vary, or there is extra non audio data between each packet.
///
/// This is necessary to describe variable bit rate formats or in cases when the
/// channels are of unequal size. In these scenarios
/// `AudioStreamPacketDescription` supplements the information in
/// `AudioStreamBasicDescription`.
#[repr(C)]
pub struct AudioStreamPacketDescription {
    /// The number of bytes from the start of the buffer to the packet
    start_offset: i64,
    /// The number of samples frames in the packet.
    /// This is 0 for formats with a constant number of frames per packet.
    variable_frames_in_packet: u32,
    /// The number of bytes in the packet.
    data_byte_size: u32,
}

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

    /// Create a new audio queue for playback.
    ///
    /// The `in_format` parameter describes the format of the audio data to be
    /// played back. Note that while compressed audio data is supported, non
    /// interleaved PCM formats are not.
    ///
    /// The callback pointer to by `in_callback_proc` will be called once the
    /// queue has finished aquiring a buffer. This callback will be passed
    /// `in_user_data`, which can be used to supply custom data.
    ///
    /// The `in_callback_run_loop` parameter can optionally be used to provide a
    /// CFRunLoop to invoke the callback. Passing null will instead result
    /// in the callback being invoked from one of the queues internal threads.
    /// If a CFRunLoop is used, `in_callback_run_loop_mode` will determine the
    /// run loop mode.
    ///
    /// Currently `in_flags` is not used, and must be set to 0.
    ///
    /// Once this function has been executed, the reference pointed to by the
    /// `out_aq` parameter will contain the newly created queue. If creating
    /// a new queue fails, an error will be returned.
    #[link_name = "AudioQueueNewOutput"]
    pub fn audio_queue_new_output(
        in_format: *const AudioStreamBasicDescription,
        in_callback_proc: AudioQueueOutputCallback,
        in_user_data: *mut c_void,
        in_callback_run_loop: CFRunLoopRef,
        in_callback_run_loop_mode: CFStringRef,
        in_flags: u32,
        out_aq: *mut AudioQueueRef,
    ) -> OSStatus;
}

/// A reference to an opaque CFURL object.
pub type CFURLRef = *const CFURL;

/// A reference to an opaque CFDictionary object.
pub type CFDictionaryRef = *const CFDictionary;

/// A reference to an opaque CFString object.
pub type CFStringRef = *const CFString;

/// A reference to an opaque CFRunLoop object.
pub type CFRunLoopRef = *const CFRunLoop;

/// Specifies a particular string encoding.
///
/// Used when interacting with CFString functions.
pub type CFStringEncoding = u32;

/// Signed integer type used throughout CoreFoundation.
pub type CFIndex = isize;

/// Indicates UTF-8 string encoding.
pub const CFSTRING_ENCODING_UTF8: CFStringEncoding = 0x08000100;

/// Representation of a range of sequential items.
#[repr(C)]
pub struct CFRange {
    pub location: CFIndex,
    pub length: CFIndex,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {

    /// Core Foundation URL, provides functionality to manipulate URL strings.
    pub type CFURL;

    /// Core Foundation Dictionary, holds data in key value pairs.
    pub type CFDictionary;

    /// Core Foundation String, exposes various string manipulation features.
    pub type CFString;

    /// Core Foundation run loop, dispatches control in response to inputs.
    pub type CFRunLoop;

    /// Get the number of key value pairs stored in a `CFDictionary`.
    ///
    /// Passing in anything other than a `CFDictionary` is undefined behavior.
    #[link_name = "CFDictionaryGetCount"]
    pub fn cfdictionary_get_count(dict: CFDictionaryRef) -> CFIndex;

    /// Extract keys and values from a CFDictionary.
    ///
    /// For the dictionary referenced by `dict`, extract its contents into
    /// buffers referenced by `keys` and `values`.
    ///
    /// These  output buffers should be C style array of pointer sized values
    /// and have enough capacity to hold the dictionary contents.
    ///
    /// Use the `cfdictionary_get_count` function to assist in constructing
    /// output buffers of the correct size.
    ///
    /// Key value pairs are output parallel, i.e pairs in the dictionary will
    /// have the same index in each respective buffer.
    ///
    /// Either buffer parameter can take a null pointer if output is not
    /// required.
    ///
    /// If any returned keys or values are Core Foundation objects then their
    /// ownership follows The Get Rule.
    ///
    /// Note, the following is considered to be undefined behaviour:
    ///  - Passing in anything other than a valid `CFDictionary` to the `dict`
    ///    parameter.
    ///  - passing in anything other than a valid pointer to an appropriately
    ///    size C style array to `keys` or `values`.
    #[link_name = "CFDictionaryGetKeysAndValues"]
    pub fn cfdictionary_get_keys_and_values(
        dict: CFDictionaryRef,
        keys: *mut *const c_void,
        values: *mut *const c_void,
    );

    /// Get the length of a string in the  UTF-16 code units.
    ///
    /// For example:
    ///  - "tree" -> 4
    ///  - "🍊tree" -> 6
    ///  - "𐑐" -> 2
    #[link_name = "CFStringGetLength"]
    pub fn cfstring_get_length(string_ref: CFStringRef) -> CFIndex;

    /// Extract a range of characters from a CFString into a buffer using a
    /// specified encoding.
    ///
    /// For the Core Foundation string referenced by the `string_ref` parameter,
    /// extract the range of characters specificed by `range`, into `buffer`
    /// using the encoding indicated by `encoding`.
    ///
    /// Note, this function requries you to follow these constraints:
    /// - The requested range must not exceed the length of the string.
    /// - The `max_buf_len` parameter should contain the size of `buffer`.
    ///
    /// The `loss_byte` parameter can be used to choose a character that is
    /// substituted for characters that can not be represented in the requested
    /// encoding. Passing `0` indicates that lossy conversion should not
    /// occur.
    ///
    /// Setting the `is_external_representation` parameter to true will
    /// potentially add a byte order marker indicating endianness.
    ///
    /// Passing null to `buffer` is permissible if you are only interested in if
    /// conversion will succeed and if so how many bytes are required.
    ///
    /// On return `used_buf_len` will hold the number of converted bytes
    /// actually in the buffer. This parameter accepts null if this information
    /// is not needed.
    ///
    /// Returns the number of characters converted.
    #[link_name = "CFStringGetBytes"]
    pub fn cfstring_get_bytes(
        string_ref: CFStringRef,
        range: CFRange,
        encoding: CFStringEncoding,
        loss_byte: u8,
        is_external_representation: bool,
        buffer: *mut u8,
        max_buf_len: CFIndex,
        used_buf_len: *mut CFIndex,
    ) -> CFIndex;

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
        buf_len: CFIndex,
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
