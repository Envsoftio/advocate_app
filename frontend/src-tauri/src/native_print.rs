//! Present the already prepared print PDF through the system print panel.

#[cfg(target_os = "macos")]
use tauri::Manager;

#[tauri::command]
pub(crate) async fn rust_prepare_print_pdf(
    backend: tauri::State<'_, crate::BackendState>,
    runtime: tauri::State<'_, crate::rust_backend::RustRuntime>,
    route: String,
    options: serde_json::Value,
) -> Result<matterdraft_backend::Reply, String> {
    if !backend.connection()?.is_rust() {
        return Err("Rust mode is not selected".into());
    }
    #[cfg(target_os = "macos")]
    {
        let fit = match options["scale"].as_str() {
            Some("fit") => true,
            Some("actual") => false,
            _ => return Err("Choose fit or actual size".into()),
        };
        runtime.register_print(route, options, fit).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (runtime, route, options);
        Err("The macOS print panel is unavailable on this platform".into())
    }
}

#[tauri::command]
pub(crate) async fn print_rust_pdf(
    backend: tauri::State<'_, crate::BackendState>,
    runtime: tauri::State<'_, crate::rust_backend::RustRuntime>,
    window: tauri::WebviewWindow,
    token: String,
) -> Result<(), String> {
    if !backend.connection()?.is_rust() {
        return Err("Rust mode is not selected".into());
    }
    #[cfg(target_os = "macos")]
    {
        let admission = macos::admit()?;
        let (document, fit) = runtime
            .with_print_bytes(&token, |bytes, fit| {
                macos::PreparedDocument::load(bytes).map(|document| (document, fit))
            })
            .await?;

        let (sender, receiver) = tokio::sync::oneshot::channel();
        let app = window.app_handle().clone();
        app.run_on_main_thread(move || {
            macos::open_panel(window, document, fit, admission, sender);
        })
        .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "The print panel closed unexpectedly")?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (runtime, window, token);
        Err("The macOS print panel is unavailable on this platform".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Manager;
    use objc2::{
        define_class, extern_class, msg_send,
        rc::{autoreleasepool, Retained},
        runtime::Bool,
        sel, AnyThread, DefinedClass, MainThreadMarker, Message,
    };
    use objc2_app_kit::{NSPrintInfo, NSPrintOperation, NSWindow};
    use objc2_foundation::{NSData, NSObject};
    use std::{cell::RefCell, ffi::c_void, ptr};
    use tokio::sync::{oneshot, Semaphore, SemaphorePermit};

    type Completion = oneshot::Sender<Result<(), String>>;
    type Admission = SemaphorePermit<'static>;
    static PRINT_ADMISSION: Semaphore = Semaphore::const_new(1);
    thread_local! {
        // Retain every AppKit object on the main thread until the delegate
        // receives completion, including when the IPC caller stops waiting.
        static ACTIVE_PRINT: RefCell<Option<ActivePrint>> = const { RefCell::new(None) };
    }

    pub(super) fn admit() -> Result<Admission, String> {
        PRINT_ADMISSION
            .try_acquire()
            .map_err(|_| "A print operation is already pending".into())
    }

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}

    extern_class!(
        #[unsafe(super(NSObject))]
        struct PDFDocument;
    );

    pub(super) struct PreparedDocument {
        document: Retained<PDFDocument>,
        _data: Retained<NSData>,
    }

    // SAFETY: PDFDocument's data initializer and page-count access have no
    // main-thread requirement. This private wrapper transfers the sole Rust
    // owner and immutable data once, after initialization, before AppKit uses
    // the document. No PDFKit object is accessed concurrently by Rust.
    unsafe impl Send for PreparedDocument {}

    impl PreparedDocument {
        pub(super) fn load(bytes: Vec<u8>) -> Result<Self, String> {
            autoreleasepool(|_| {
                let data = NSData::from_vec(bytes);
                // SAFETY: Invoke PDFKit's declared initializer with retained data.
                let document: Option<Retained<PDFDocument>> =
                    unsafe { msg_send![PDFDocument::alloc(), initWithData: &*data] };
                let document = document.ok_or("macOS could not read the prepared print PDF")?;
                // SAFETY: PDFKit declares pageCount as NSUInteger.
                let count: usize = unsafe { msg_send![&*document, pageCount] };
                if count == 0 {
                    return Err("The prepared print PDF contains no pages".into());
                }
                Ok(Self {
                    document,
                    _data: data,
                })
            })
        }
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "MatterDraftPrintDelegate"]
        #[ivars = tauri::AppHandle]
        struct PrintDelegate;

        impl PrintDelegate {
            #[unsafe(method(printOperationDidRun:success:contextInfo:))]
            fn did_run(&self, _operation: &NSPrintOperation, _success: Bool, _context: *mut c_void) {
                // AppKit can invoke this selector on its printing thread.
                let delegate = self.retain();
                let app = delegate.ivars().clone();
                let _ = app.run_on_main_thread(|| {
                    let active = ACTIVE_PRINT.with(|slot| slot.take());
                    if let Some(active) = active {
                        // The established command treats a normal print cancellation as closing the panel.
                        let _ = active.completion.send(Ok(()));
                    }
                });
            }
        }
    );

    struct ActivePrint {
        _admission: Admission,
        _document: PreparedDocument,
        _window: Retained<NSWindow>,
        _operation: Retained<NSPrintOperation>,
        _delegate: Retained<PrintDelegate>,
        completion: Completion,
    }

    pub(super) fn open_panel(
        window: tauri::WebviewWindow,
        document: PreparedDocument,
        fit: bool,
        admission: Admission,
        completion: Completion,
    ) {
        let (operation, delegate, native_window) = match prepare_panel(&window, &document, fit) {
            Ok(panel) => panel,
            Err(error) => {
                let _ = completion.send(Err(error));
                return;
            }
        };
        ACTIVE_PRINT.with(|slot| {
            *slot.borrow_mut() = Some(ActivePrint {
                _admission: admission,
                _document: document,
                _window: native_window.clone(),
                _operation: operation.clone(),
                _delegate: delegate.clone(),
                completion,
            });
        });
        // SAFETY: The native window and delegate are retained until completion.
        // The selector matches AppKit's documented callback signature.
        unsafe {
            operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                &native_window,
                Some(delegate.as_ref()),
                Some(sel!(printOperationDidRun:success:contextInfo:)),
                ptr::null_mut(),
            );
        }
    }

    fn prepare_panel(
        window: &tauri::WebviewWindow,
        document: &PreparedDocument,
        fit: bool,
    ) -> Result<
        (
            Retained<NSPrintOperation>,
            Retained<PrintDelegate>,
            Retained<NSWindow>,
        ),
        String,
    > {
        let main = MainThreadMarker::new().ok_or("The print panel requires the main thread")?;
        if ACTIVE_PRINT.with(|slot| slot.borrow().is_some())
            || NSPrintOperation::currentOperation(main).is_some()
        {
            return Err("A print operation is already pending".into());
        }
        let native_window = window.ns_window().map_err(|error| error.to_string())?;
        // SAFETY: On macOS Tauri returns the live AppKit NSWindow handle. Retain
        // it here on the main thread for the lifetime of the attached sheet.
        let native_window = unsafe { Retained::retain(native_window.cast::<NSWindow>()) }
            .ok_or("The print window is unavailable")?;
        let info = NSPrintInfo::sharedPrintInfo();
        // PDFPrintScalingMode uses NSInteger. 0 preserves size and 1 fits.
        let scaling_mode: isize = if fit { 1 } else { 0 };
        // SAFETY: The PDFKit selector is called on the main thread with its
        // declared argument types and a retained PDFDocument.
        let operation: Option<Retained<NSPrintOperation>> = unsafe {
            msg_send![
                &*document.document,
                printOperationForPrintInfo: &*info,
                scalingMode: scaling_mode,
                autoRotate: Bool::NO
            ]
        };
        let operation = operation.ok_or("macOS could not prepare the print panel")?;
        operation.setShowsPrintPanel(true);
        operation.setCanSpawnSeparateThread(true);
        // SAFETY: PrintDelegate is an NSObject subclass and inherits init.
        let delegate = unsafe {
            msg_send![
                super(PrintDelegate::alloc().set_ivars(window.app_handle().clone())),
                init
            ]
        };
        Ok((operation, delegate, native_window))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn prepared_print_keeps_admission_across_worker_handoff() {
            let bytes = include_bytes!("../../../samples/01_petition.pdf");
            let admission = admit().unwrap();
            let handoff = std::thread::spawn(move || {
                (PreparedDocument::load(bytes.to_vec()).unwrap(), admission)
            })
            .join()
            .unwrap();
            assert!(admit().is_err());
            drop(handoff);
            assert!(admit().is_ok());
        }
    }
}
