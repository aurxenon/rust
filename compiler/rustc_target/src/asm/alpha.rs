use super::{InlineAsmArch, InlineAsmType};
use rustc_macros::HashStable_Generic;
use rustc_span::{Symbol};
use std::fmt;

def_reg_class! {
    Alpha AlphaInlineAsmRegClass {
        reg,
        freg,
    }
}

impl AlphaInlineAsmRegClass {
    pub fn valid_modifiers(self, _arch: super::InlineAsmArch) -> &'static [char] {
        &[]
    }

    pub fn suggest_class(self, _arch: InlineAsmArch, _ty: InlineAsmType) -> Option<Self> {
        None
    }

    pub fn suggest_modifier(
        self,
        _arch: InlineAsmArch,
        _ty: InlineAsmType,
    ) -> Option<(char, &'static str)> {
        None
    }

    pub fn default_modifier(self, _arch: InlineAsmArch) -> Option<(char, &'static str)> {
        None
    }

    pub fn supported_types(
        self,
        _arch: InlineAsmArch,
    ) -> &'static [(InlineAsmType, Option<Symbol>)] {
        match self {
            Self::reg => {
                types! { _: I8, I16, I32, I64, F32, F64; }
            }
            Self::freg => types! { f: F32; d: F64; },
        }
    }
}

def_regs! {
    Alpha AlphaInlineAsmReg AlphaInlineAsmRegClass {
        r0: reg = ["$0", "v0"],
        r1: reg = ["$1", "t0"],
        r2: reg = ["$2", "t1"],
        r3: reg = ["$3", "t2"],
        r4: reg = ["$4", "t3"],
        r5: reg = ["$5", "t4"],
        r6: reg = ["$6", "t5"],
        r7: reg = ["$7", "t6"],
        r8: reg = ["$8", "t7"],
        r9: reg = ["$9", "s0"],
        r10: reg = ["$10", "s1"],
        r11: reg = ["$11", "s2"],
        r12: reg = ["$12", "s3"],
        r13: reg = ["$13", "s4"],
        r14: reg = ["$14", "s5"],
        r16: reg = ["$16", "a0"],
        r17: reg = ["$17", "a1"],
        r18: reg = ["$18", "a2"],
        r19: reg = ["$19", "a3"],
        r20: reg = ["$20", "a4"],
        r21: reg = ["$21", "a5"],
        r22: reg = ["$22", "t8"],
        r23: reg = ["$23", "t9"],
        r24: reg = ["$24", "t10"],
        r25: reg = ["$25", "t11"],
        r26: reg = ["$26", "ra"],
        r27: reg = ["$27", "pv", "t12"],
        f0: freg = ["$f0"],
        f1: freg = ["$f1"],
        f2: freg = ["$f2"],
        f3: freg = ["$f3"],
        f4: freg = ["$f4"],
        f5: freg = ["$f5"],
        f6: freg = ["$f6"],
        f7: freg = ["$f7"],
        f8: freg = ["$f8"],
        f9: freg = ["$f9"],
        f10: freg = ["$f10"],
        f11: freg = ["$f11"],
        f12: freg = ["$f12"],
        f13: freg = ["$f13"],
        f14: freg = ["$f14"],
        f15: freg = ["$f15"],
        f16: freg = ["$f16"],
        f17: freg = ["$f17"],
        f18: freg = ["$f18"],
        f19: freg = ["$f19"],
        f20: freg = ["$f20"],
        f21: freg = ["$f21"],
        f22: freg = ["$f22"],
        f23: freg = ["$f23"],
        f24: freg = ["$f24"],
        f25: freg = ["$f25"],
        f26: freg = ["$f26"],
        f27: freg = ["$f27"],
        f28: freg = ["$f28"],
        f29: freg = ["$f29"],
        f30: freg = ["$f30"],
        #error = ["$15", "fp", "s6"] =>
            "the frame pointer cannot be used as an operand for inline asm",
        #error = ["$30", "sp"] =>
            "the stack pointer cannot be used as an operand for inline asm",
        #error = ["$29", "gp"] =>
            "the global pointer cannot be used as an operand for inline asm",
        #error = ["$28", "at"] =>
            "the assembler temporary pointer cannot be used as an operand for inline asm" ,
        #error = ["$31", "zero"] =>
            "the zero register cannot be used as an operand for inline asm",
        #error = ["$f31"] =>
            "the floating point zero register cannot be used as an operand for inline asm",
    }
}

impl AlphaInlineAsmReg {
    pub fn emit(
        self,
        out: &mut dyn fmt::Write,
        _arch: InlineAsmArch,
        _modifier: Option<char>,
    ) -> fmt::Result {
        out.write_str(self.name())
    }
}
