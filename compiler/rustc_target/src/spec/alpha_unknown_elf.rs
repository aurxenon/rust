use crate::spec::{Cc, LinkerFlavor, Lld, PanicStrategy};
use crate::spec::{RelocModel, Target, TargetOptions};

pub fn target() -> Target {
    Target {
        data_layout: "e-m:e-S128-p:64:64-i32:32:32-i64:64:64-a:0-n32:64".into(),
        llvm_target: "alpha".into(),
        pointer_width: 64,
        arch: "alpha".into(),

        options: TargetOptions {
            linker_flavor: LinkerFlavor::Gnu(Cc::Yes, Lld::No),
            linker: Some("alpha-unknown-elf-gcc".into()),
            cpu: "ev56".into(),
            max_atomic_width: Some(64),
            panic_strategy: PanicStrategy::Abort,
            relocation_model: RelocModel::Static,
            emit_debug_gdb_scripts: false,
            eh_frame_header: false,
            ..Default::default()
        },
    }
}
