//! Exposes the shared application protocol through JNI without MATSim dependencies in the client.
use jni::{
    JNIEnv,
    objects::{JClass, JString},
    sys::{jint, jlong, jstring},
};

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_apiVersion(
    _: JNIEnv,
    _: JClass,
) -> jint {
    mmrouter_service::CLIENT_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_openInput(
    mut env: JNIEnv,
    _: JClass,
    path: JString,
    format: JString,
    parameters: JString,
) -> jlong {
    super::boundary(&mut env, |env| {
        let path: String = env.get_string(&path)?.into();
        let format: String = env.get_string(&format)?.into();
        let parameters: String = env.get_string(&parameters)?.into();
        mmrouter_service::open_input(&path, &format, serde_json::from_str(&parameters)?)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_protocolVersion(
    _: JNIEnv,
    _: JClass,
) -> jint {
    mmrouter_service::PROTOCOL_VERSION
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_eqasim_switzerland_ch_1cmdp_rustRouting_NativePtRouter_protocolVersion(
    _: JNIEnv,
    _: JClass,
) -> jint {
    mmrouter_service::PROTOCOL_VERSION
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_openConfig(
    mut env: JNIEnv,
    _: JClass,
    path: JString,
) -> jlong {
    super::boundary(&mut env, |env| {
        let path: String = env.get_string(&path)?.into();
        mmrouter_service::open(&path)
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_request(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    input: JString,
) -> jstring {
    super::boundary(&mut env, |env| {
        let input: String = env.get_string(&input)?.into();
        Ok(env
            .new_string(mmrouter_service::request(handle, &input))?
            .into_raw())
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_multimodalmatsim_router_NativeRouter_release(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    super::boundary(&mut env, |_| mmrouter_service::close(handle));
}
