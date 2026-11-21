//! In-process Tesseract C API. Models and the library are local to the host.
use super::*;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

pub(super) const UNAVAILABLE: &str = "Tesseract is unavailable on this device";
struct Runtime {
    root: PathBuf,
    required: bool,
}

static BUNDLED_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub(super) fn set_resource_root(root: PathBuf, required: bool) {
    if root.is_absolute() {
        let _ = BUNDLED_RUNTIME.set(Runtime {
            root: root.join("matterdraft-ocr"),
            required,
        });
    }
}

type Handle = *mut c_void;
type Destroy = unsafe extern "C" fn(Handle);
type Create = unsafe extern "C" fn() -> Handle;

struct OwnedHandle(Handle, Destroy);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // The library remains loaded for the lifetime of each handle.
        unsafe { (self.1)(self.0) };
    }
}

pub(super) struct Engine {
    handle: OwnedHandle,
    library: Library,
    clear_adaptation: Option<Destroy>,
    pub provenance: Value,
}

pub(super) struct Word {
    pub text: String,
    pub confidence: f64,
    pub rect: [f64; 4],
}

unsafe extern "C" fn cancellation(context: *mut c_void, _words: c_int) -> bool {
    unsafe { (*context.cast::<AtomicBool>()).load(Ordering::Acquire) }
}

fn library_path() -> Vec<PathBuf> {
    let bundled = BUNDLED_RUNTIME.get();
    let required = bundled.is_some_and(|runtime| runtime.required);
    if let Some(path) = std::env::var_os("MATTERDRAFT_TESSERACT_LIBRARY").filter(|_| !required) {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let files: &[&str] = if cfg!(target_os = "macos") {
        &["libtesseract.5.dylib"]
    } else if cfg!(target_os = "windows") {
        &[
            "tesseract55.dll",
            "tesseract54.dll",
            "tesseract53.dll",
            "tesseract52.dll",
            "tesseract51.dll",
            "tesseract50.dll",
            "libtesseract-5.dll",
            "libtesseract.dll",
            "tesseract.dll",
        ]
    } else {
        &["libtesseract.so.5"]
    };
    let mut paths = bundled
        .into_iter()
        .flat_map(|runtime| {
            files
                .iter()
                .map(move |file| runtime.root.join("lib").join(file))
        })
        .collect::<Vec<_>>();
    if let Some(bundled) = bundled {
        let entry = File::open(bundled.root.join("manifest.json"))
            .ok()
            .and_then(|file| serde_json::from_reader::<_, Value>(file.take(64 * 1024)).ok())
            .and_then(|manifest| manifest["entry"].as_str().map(str::to_owned));
        if let Some(entry) = entry
            .filter(|entry| entry.starts_with("tesseract") || entry.starts_with("libtesseract"))
            .filter(|entry| {
                entry
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    && if cfg!(windows) {
                        entry.ends_with(".dll")
                    } else if cfg!(target_os = "macos") {
                        entry.ends_with(".dylib")
                    } else {
                        entry.ends_with(".so.5")
                    }
            })
        {
            let path = bundled.root.join("lib").join(entry);
            if !paths.contains(&path) {
                paths.insert(0, path);
            }
        }
    }
    if required {
        return paths;
    }
    #[cfg(target_os = "macos")]
    paths.extend([
        "/opt/homebrew/lib/libtesseract.5.dylib".into(),
        "/usr/local/lib/libtesseract.5.dylib".into(),
    ] as [PathBuf; 2]);
    #[cfg(target_os = "linux")]
    paths.extend([
        "/usr/lib/aarch64-linux-gnu/libtesseract.so.5".into(),
        "/usr/lib/x86_64-linux-gnu/libtesseract.so.5".into(),
        "/usr/lib64/libtesseract.so.5".into(),
    ] as [PathBuf; 3]);
    paths
}

fn model_root() -> Option<PathBuf> {
    let bundled = BUNDLED_RUNTIME.get();
    if let Some(runtime) = bundled.filter(|runtime| runtime.required) {
        let models = runtime.root.join("tessdata");
        return models.is_dir().then_some(models);
    }
    let configured =
        std::env::var_os("MATTERDRAFT_TESSDATA").or_else(|| std::env::var_os("TESSDATA_PREFIX"));
    if let Some(path) = configured {
        let path = PathBuf::from(path);
        return (path.is_absolute() && path.is_dir()).then_some(path);
    }
    if let Some(runtime) = bundled {
        let models = runtime.root.join("tessdata");
        if models.is_dir() {
            return Some(models);
        }
    }
    [
        "/opt/homebrew/share/tessdata",
        "/usr/local/share/tessdata",
        "/usr/share/tesseract-ocr/5/tessdata",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_dir())
}

fn string(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

struct Library(Handle);

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(path: *const u16, reserved: Handle, flags: u32) -> Handle;
    fn GetProcAddress(module: Handle, name: *const c_char) -> Handle;
    fn FreeLibrary(module: Handle) -> i32;
}

impl Library {
    fn open(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        let module = {
            use std::os::unix::ffi::OsStrExt;
            let path = CString::new(path.as_os_str().as_bytes()).ok()?;
            unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LOCAL | libc::RTLD_NOW) }
        };
        #[cfg(windows)]
        let module = {
            use std::os::windows::ffi::OsStrExt;
            const DLL_LOAD_DIR: u32 = 0x100;
            const DEFAULT_DIRS: u32 = 0x1000;
            let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
            if path.contains(&0) {
                return None;
            }
            path.push(0);
            unsafe {
                LoadLibraryExW(
                    path.as_ptr(),
                    std::ptr::null_mut(),
                    DLL_LOAD_DIR | DEFAULT_DIRS,
                )
            }
        };
        #[cfg(not(any(unix, windows)))]
        let module: Handle = std::ptr::null_mut();
        (!module.is_null()).then(|| Self(module))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::dlclose(self.0);
        }
        #[cfg(windows)]
        unsafe {
            FreeLibrary(self.0);
        }
    }
}

fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, &'static str> {
    let name = CStr::from_bytes_with_nul(name).map_err(|_| UNAVAILABLE)?;
    #[cfg(unix)]
    let address = unsafe { libc::dlsym(library.0, name.as_ptr()) };
    #[cfg(windows)]
    let address = unsafe { GetProcAddress(library.0, name.as_ptr()) };
    #[cfg(not(any(unix, windows)))]
    let address: Handle = std::ptr::null_mut();
    if address.is_null() || std::mem::size_of::<T>() != std::mem::size_of::<Handle>() {
        return Err(UNAVAILABLE);
    }
    // Every caller provides the exact Tesseract C signature for this symbol.
    Ok(unsafe { std::mem::transmute_copy(&address) })
}

impl Engine {
    pub fn new(language: &str) -> Result<Self, &'static str> {
        if !matches!(language, "eng" | "hin" | "eng+hin") {
            return Err("Unsupported OCR language");
        }
        let library = library_path()
            .iter()
            .find_map(|path| Library::open(path))
            .ok_or(UNAVAILABLE)?;
        let root = model_root().ok_or("Tesseract OCR language models are unavailable")?;
        let path = CString::new(root.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid OCR model location")?;
        let label = CString::new(language).map_err(|_| "Unsupported OCR language")?;
        let mut models = serde_json::Map::new();
        for model in language.split('+') {
            let file = File::open(root.join(format!("{model}.traineddata")))
                .map_err(|_| "Tesseract OCR language model is missing")?;
            const LIMIT: u64 = 64 * 1024 * 1024;
            let mut digest = Sha256::new();
            let mut read = std::io::BufReader::new(file).take(LIMIT + 1);
            let size = std::io::copy(&mut read, &mut HashSink(&mut digest))
                .map_err(|_| "Cannot read OCR model")?;
            if size == 0 || size > LIMIT {
                return Err("OCR model exceeds the supported size");
            }
            models.insert(
                model.to_owned(),
                json!(vault::hex_bytes(&digest.finalize()).as_str()),
            );
        }
        let create: Create = symbol(&library, b"TessBaseAPICreate\0")?;
        let destroy: Destroy = symbol(&library, b"TessBaseAPIDelete\0")?;
        let init: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int =
            symbol(&library, b"TessBaseAPIInit3\0")?;
        let version: unsafe extern "C" fn() -> *const c_char = symbol(&library, b"TessVersion\0")?;
        let handle = unsafe { create() };
        if handle.is_null() {
            return Err(UNAVAILABLE);
        }
        let handle = OwnedHandle(handle, destroy);
        if unsafe { init(handle.0, path.as_ptr(), label.as_ptr()) } != 0 {
            return Err("Cannot initialize the selected Tesseract OCR language");
        }
        Ok(Self {
            handle,
            library,
            clear_adaptation: None,
            provenance: json!({"provider":"Tesseract","version":string(unsafe { version() }),
                "language":language,"traineddata_sha256":models,"dpi":240,"text_verified":false}),
        })
    }

    pub fn for_measurement(language: &str) -> Result<Self, &'static str> {
        const PSM_AUTO: c_int = 3;
        const PSM_SINGLE_BLOCK: c_int = 6;
        let mut engine = Self::new(language)?;
        let get_mode: unsafe extern "C" fn(Handle) -> c_int =
            symbol(&engine.library, b"TessBaseAPIGetPageSegMode\0")?;
        let set_mode: unsafe extern "C" fn(Handle, c_int) =
            symbol(&engine.library, b"TessBaseAPISetPageSegMode\0")?;
        // Use the CLI default while keeping any mode selected by the model.
        unsafe {
            if get_mode(engine.handle.0) == PSM_SINGLE_BLOCK {
                set_mode(engine.handle.0, PSM_AUTO);
            }
        }
        // LSTM-only builds can omit the legacy adaptive classifier entirely.
        engine.clear_adaptation =
            symbol(&engine.library, b"TessBaseAPIClearAdaptiveClassifier\0").ok();
        Ok(engine)
    }

    pub fn recognize(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Word>, &'static str> {
        if cancelled.load(Ordering::Acquire) {
            return Err("Operation cancelled");
        }
        let library = &self.library;
        let set_image: unsafe extern "C" fn(Handle, *const u8, c_int, c_int, c_int, c_int) =
            symbol(library, b"TessBaseAPISetImage\0")?;
        let dpi: unsafe extern "C" fn(Handle, c_int) =
            symbol(library, b"TessBaseAPISetSourceResolution\0")?;
        let recognize: unsafe extern "C" fn(Handle, Handle) -> c_int =
            symbol(library, b"TessBaseAPIRecognize\0")?;
        let tsv: unsafe extern "C" fn(Handle, c_int) -> *const c_char =
            symbol(library, b"TessBaseAPIGetTsvText\0")?;
        let free: unsafe extern "C" fn(*const c_char) = symbol(library, b"TessDeleteText\0")?;
        let create: Create = symbol(library, b"TessMonitorCreate\0")?;
        let destroy: Destroy = symbol(library, b"TessMonitorDelete\0")?;
        let set_cancel: unsafe extern "C" fn(
            Handle,
            unsafe extern "C" fn(*mut c_void, c_int) -> bool,
        ) = symbol(library, b"TessMonitorSetCancelFunc\0")?;
        let cancel_this: unsafe extern "C" fn(Handle, *mut c_void) =
            symbol(library, b"TessMonitorSetCancelThis\0")?;
        let deadline: unsafe extern "C" fn(Handle, c_int) =
            symbol(library, b"TessMonitorSetDeadlineMSecs\0")?;
        if u64::from(width) * u64::from(height) * 4 != rgba.len() as u64
            || width == 0
            || height == 0
        {
            return Err("Invalid raster for OCR");
        }
        let bytes = rgba
            .chunks_exact(4)
            .flat_map(|pixel| {
                let alpha = u16::from(pixel[3]);
                [pixel[0], pixel[1], pixel[2]]
                    .map(|value| ((u16::from(value) * alpha + 255 * (255 - alpha)) / 255) as u8)
            })
            .collect::<Vec<_>>();
        let monitor = unsafe { create() };
        if monitor.is_null() {
            return Err("Unable to prepare OCR cancellation");
        }
        let monitor = OwnedHandle(monitor, destroy);
        let handle = self.handle.0;
        unsafe {
            if let Some(reset) = self.clear_adaptation {
                reset(handle);
            }
            set_cancel(monitor.0, cancellation);
            cancel_this(monitor.0, std::ptr::from_ref(cancelled).cast_mut().cast());
            deadline(monitor.0, 90_000);
            set_image(
                handle,
                bytes.as_ptr(),
                width as c_int,
                height as c_int,
                3,
                (width * 3) as c_int,
            );
            dpi(handle, 240);
            if recognize(handle, monitor.0) != 0 {
                return Err(if cancelled.load(Ordering::Acquire) {
                    "Operation cancelled"
                } else {
                    "Tesseract could not recognize this page"
                });
            }
            let pointer = tsv(handle, 0);
            let text = string(pointer).ok_or("Tesseract did not return confidence data")?;
            free(pointer);
            if text.len() > 8 * 1024 * 1024 {
                return Err("OCR result exceeds the page text limit");
            }
            return words(&text, width, height);
        }
    }
}

fn words(tsv: &str, width: u32, height: u32) -> Result<Vec<Word>, &'static str> {
    let mut result = Vec::new();
    for line in tsv.lines() {
        let fields = line.splitn(12, '\t').collect::<Vec<_>>();
        if fields.len() != 12 || fields[0] != "5" || fields[11].trim().is_empty() {
            continue;
        }
        let mut rect = [0.0; 4];
        for (value, field) in rect.iter_mut().zip(&fields[6..10]) {
            *value = field
                .parse::<f64>()
                .map_err(|_| "Tesseract returned an invalid position")?;
        }
        let confidence = fields[10]
            .parse::<f64>()
            .map_err(|_| "Tesseract returned invalid confidence")?;
        if !(0.0..=100.0).contains(&confidence)
            || rect.iter().any(|value| !value.is_finite() || *value < 0.0)
            || rect[0] + rect[2] > f64::from(width)
            || rect[1] + rect[3] > f64::from(height)
        {
            continue;
        }
        for (index, value) in rect.iter_mut().enumerate() {
            *value /= f64::from(if index % 2 == 0 { width } else { height });
        }
        result.push(Word {
            text: fields[11].trim().to_owned(),
            confidence,
            rect,
        });
    }
    Ok(result)
}

struct HashSink<'a>(&'a mut Sha256);

impl Write for HashSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
