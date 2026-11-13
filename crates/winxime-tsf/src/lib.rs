// TSF COM DLL 必须导出 DllGetClassObject / DllCanUnloadNow / DllRegisterServer /
// DllUnregisterServer（见 dll.rs 的 #[no_mangle]）。链接器对 cdylib 的非 PRIVATE
// 导出统一报 LNK4104（导出符号应为 PRIVATE），属于本 DLL 形态的正常现象而非
// 缺陷，故在 crate 级关闭 rustc 对链接器输出的告警透传。
#![allow(linker_messages)]

mod class_factory;
mod dll;
mod language_bar;
mod text_input_processor;

pub use class_factory::ClassFactory;
pub use language_bar::{set_instance, LangBarItemButton};
pub use text_input_processor::XimeTextService;
