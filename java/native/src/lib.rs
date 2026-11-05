//! Shares panic containment across the standalone Java and specialized MATSim JNI entry points.
#[path = "../../matsim/native/bridge.rs"]
mod matsim;
mod standalone;
use anyhow::Result;
use jni::JNIEnv;
use std::panic::{AssertUnwindSafe, catch_unwind};
/// No panic may unwind through a JVM frame. JNI exceptions preserve useful validation messages.
fn boundary<T: Default>(env: &mut JNIEnv, operation: impl FnOnce(&mut JNIEnv) -> Result<T>) -> T {
    match catch_unwind(AssertUnwindSafe(|| operation(env))) {
        Ok(Ok(value)) => value,
        result => {
            let message = match result {
                Ok(Err(error)) => format!("{error:#}"),
                _ => "Rust PT router panicked".into(),
            };
            if !env.exception_check().unwrap_or(true) {
                let _ = env.throw_new("java/lang/IllegalStateException", message);
            }
            T::default()
        }
    }
}
