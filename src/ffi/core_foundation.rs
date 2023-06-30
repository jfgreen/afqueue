//! Selected FFI bindings to CoreFoundation.

// Note: Core foundation uses a `Boolean` type which is typedef as `unsigned
// char`. These bindings work on the assumption that a Rust `bool` is ABI
// compatable with this heratage Carbon era type.

use std::ffi::c_void;

/// A type free reference to an opaque Core Foundation object.
///
/// This type is accepted by polymorphic functions like `cf_release`.
pub type CFTypeRef = *const c_void;

/// Unique identifer of a Core Foundation opaque type.
pub type CFTypeID = usize;

/// A reference to a CFAllocator object.
///
/// CFAllocatorRef is used in many Core Foundation parameters which need to
/// allocate memory. For our use case, we can supply an null pointer to tell
/// Core Foundation to use the default allocator.
pub type CFAllocatorRef = *const c_void;

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

/// Opaque data type for Core Foundation URL, provides functionality to
/// manipulate URL strings.
pub enum CFURL {}

/// Opaque data type for Core Foundation Dictionary, holds data in key value
/// pairs.
pub enum CFDictionary {}

/// Opaque data type for Core Foundation String, exposes various string
/// manipulation features.
pub enum CFString {}

/// Opaque data type for Core Foundation run loop, dispatches control in
/// response to inputs.
pub enum CFRunLoop {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {

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

    /// Get the length of a string in UTF-16 code units.
    ///
    /// Note: Due to how UTF-16 employs surrogate pairs, what is visually
    /// rendered as a single character might be more than one code unit.
    ///
    /// For example:
    ///  - "tree" -> 4
    ///  - "🍊tree" -> 6
    ///  - "𐑐" -> 2
    ///  - "🧘🏻‍♂️" -> 7
    #[link_name = "CFStringGetLength"]
    pub fn cfstring_get_length(string_ref: CFStringRef) -> CFIndex;

    /// Extract a range of UTF-16 code units from a CFString into a buffer using
    /// a specified encoding.
    ///
    /// For the Core Foundation string referenced by the `string_ref` parameter,
    /// extract the range of characters specified by `range`, into `buffer`
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

    /// Get the unique identifer for the CFString type
    #[link_name = "CFStringGetTypeID"]
    pub fn cfstring_get_type_id() -> CFTypeID;

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

    /// Get a unique identifier indicating the type of any Core Foundation
    /// object.
    #[link_name = "CFGetTypeID"]
    pub fn cf_get_type_id(cf: CFTypeRef) -> CFTypeID;

}
