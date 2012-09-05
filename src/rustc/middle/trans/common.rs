/**
   Code that is useful in various trans modules.

*/

use libc::c_uint;
use vec::unsafe::to_ptr;
use std::map::{hashmap,set};
use syntax::{ast, ast_map};
use driver::session;
use session::session;
use middle::ty;
use back::{link, abi, upcall};
use syntax::codemap::span;
use lib::llvm::{llvm, target_data, type_names, associate_type,
                   name_has_type};
use lib::llvm::{ModuleRef, ValueRef, TypeRef, BasicBlockRef, BuilderRef};
use lib::llvm::{True, False, Bool};
use metadata::{csearch};
use metadata::common::link_meta;
use syntax::ast_map::path;
use util::ppaux::ty_to_str;
use syntax::parse::token::ident_interner;
use syntax::ast::ident;

type namegen = fn@(~str) -> ident;
fn new_namegen(intr: ident_interner) -> namegen {
    return fn@(prefix: ~str) -> ident {
        return intr.gensym(@fmt!("%s_%u", prefix, intr.gensym(@prefix)))
    };
}

type addrspace = c_uint;

// Address spaces communicate to LLVM which destructors need to run for
// specifc types.
//    0 is ignored by the GC, and is used for all non-GC'd pointers.
//    1 is for opaque GC'd boxes.
//    >= 2 are for specific types (e.g. resources).
const default_addrspace: addrspace = 0;
const gc_box_addrspace: addrspace = 1;

type addrspace_gen = fn@() -> addrspace;
fn new_addrspace_gen() -> addrspace_gen {
    let i = @mut 1;
    return fn@() -> addrspace { *i += 1; *i };
}

type tydesc_info =
    {ty: ty::t,
     tydesc: ValueRef,
     size: ValueRef,
     align: ValueRef,
     addrspace: addrspace,
     mut take_glue: Option<ValueRef>,
     mut drop_glue: Option<ValueRef>,
     mut free_glue: Option<ValueRef>,
     mut visit_glue: Option<ValueRef>};

/*
 * A note on nomenclature of linking: "extern", "foreign", and "upcall".
 *
 * An "extern" is an LLVM symbol we wind up emitting an undefined external
 * reference to. This means "we don't have the thing in this compilation unit,
 * please make sure you link it in at runtime". This could be a reference to
 * C code found in a C library, or rust code found in a rust crate.
 *
 * Most "externs" are implicitly declared (automatically) as a result of a
 * user declaring an extern _module_ dependency; this causes the rust driver
 * to locate an extern crate, scan its compilation metadata, and emit extern
 * declarations for any symbols used by the declaring crate.
 *
 * A "foreign" is an extern that references C (or other non-rust ABI) code.
 * There is no metadata to scan for extern references so in these cases either
 * a header-digester like bindgen, or manual function prototypes, have to
 * serve as declarators. So these are usually given explicitly as prototype
 * declarations, in rust code, with ABI attributes on them noting which ABI to
 * link via.
 *
 * An "upcall" is a foreign call generated by the compiler (not corresponding
 * to any user-written call in the code) into the runtime library, to perform
 * some helper task such as bringing a task to life, allocating memory, etc.
 *
 */

type stats =
    {mut n_static_tydescs: uint,
     mut n_glues_created: uint,
     mut n_null_glues: uint,
     mut n_real_glues: uint,
     llvm_insn_ctxt: @mut ~[~str],
     llvm_insns: hashmap<~str, uint>,
     fn_times: @mut ~[{ident: ~str, time: int}]};

struct BuilderRef_res {
    let B: BuilderRef;
    new(B: BuilderRef) { self.B = B; }
    drop { llvm::LLVMDisposeBuilder(self.B); }
}

// Crate context.  Every crate we compile has one of these.
type crate_ctxt = {
     sess: session::session,
     llmod: ModuleRef,
     td: target_data,
     tn: type_names,
     externs: hashmap<~str, ValueRef>,
     intrinsics: hashmap<~str, ValueRef>,
     item_vals: hashmap<ast::node_id, ValueRef>,
     exp_map: resolve::ExportMap,
     exp_map2: resolve::ExportMap2,
     reachable: reachable::map,
     item_symbols: hashmap<ast::node_id, ~str>,
     mut main_fn: Option<ValueRef>,
     link_meta: link_meta,
     enum_sizes: hashmap<ty::t, uint>,
     discrims: hashmap<ast::def_id, ValueRef>,
     discrim_symbols: hashmap<ast::node_id, ~str>,
     tydescs: hashmap<ty::t, @tydesc_info>,
     // Set when running emit_tydescs to enforce that no more tydescs are
     // created.
     mut finished_tydescs: bool,
     // Track mapping of external ids to local items imported for inlining
     external: hashmap<ast::def_id, Option<ast::node_id>>,
     // Cache instances of monomorphized functions
     monomorphized: hashmap<mono_id, ValueRef>,
     monomorphizing: hashmap<ast::def_id, uint>,
     // Cache computed type parameter uses (see type_use.rs)
     type_use_cache: hashmap<ast::def_id, ~[type_use::type_uses]>,
     // Cache generated vtables
     vtables: hashmap<mono_id, ValueRef>,
     // Cache of constant strings,
     const_cstr_cache: hashmap<~str, ValueRef>,
     // Reverse-direction for const ptrs cast from globals,
     // since the ptr -> init association is lost any
     // time a GlobalValue is cast.
     const_globals: hashmap<int, ValueRef>,
     module_data: hashmap<~str, ValueRef>,
     lltypes: hashmap<ty::t, TypeRef>,
     names: namegen,
     next_addrspace: addrspace_gen,
     symbol_hasher: @hash::State,
     type_hashcodes: hashmap<ty::t, ~str>,
     type_short_names: hashmap<ty::t, ~str>,
     all_llvm_symbols: set<~str>,
     tcx: ty::ctxt,
     maps: astencode::maps,
     stats: stats,
     upcalls: @upcall::upcalls,
     rtcalls: hashmap<~str, ast::def_id>,
     tydesc_type: TypeRef,
     int_type: TypeRef,
     float_type: TypeRef,
     task_type: TypeRef,
     opaque_vec_type: TypeRef,
     builder: BuilderRef_res,
     shape_cx: shape::ctxt,
     crate_map: ValueRef,
     dbg_cx: Option<debuginfo::debug_ctxt>,
     // Mapping from class constructors to parent class --
     // used in base::trans_closure
     // parent_class must be a def_id because ctors can be
     // inlined, so the parent may be in a different crate
     class_ctors: hashmap<ast::node_id, ast::def_id>,
     mut do_not_commit_warning_issued: bool};

// Types used for llself.
type val_self_data = {v: ValueRef, t: ty::t, is_owned: bool};

enum local_val { local_mem(ValueRef), local_imm(ValueRef), }

type param_substs = {tys: ~[ty::t],
                     vtables: Option<typeck::vtable_res>,
                     bounds: @~[ty::param_bounds]};

// Function context.  Every LLVM function we create will have one of
// these.
type fn_ctxt = @{
    // The ValueRef returned from a call to llvm::LLVMAddFunction; the
    // address of the first instruction in the sequence of
    // instructions for this function that will go in the .text
    // section of the executable we're generating.
    llfn: ValueRef,

    // The two implicit arguments that arrive in the function we're creating.
    // For instance, foo(int, int) is really foo(ret*, env*, int, int).
    llenv: ValueRef,
    llretptr: ValueRef,

    // These elements: "hoisted basic blocks" containing
    // administrative activities that have to happen in only one place in
    // the function, due to LLVM's quirks.
    // A block for all the function's static allocas, so that LLVM
    // will coalesce them into a single alloca call.
    mut llstaticallocas: BasicBlockRef,
    // A block containing code that copies incoming arguments to space
    // already allocated by code in one of the llallocas blocks.
    // (LLVM requires that arguments be copied to local allocas before
    // allowing most any operation to be performed on them.)
    mut llloadenv: BasicBlockRef,
    mut llreturn: BasicBlockRef,
    // The 'self' value currently in use in this function, if there
    // is one.
    mut llself: Option<val_self_data>,
    // The a value alloca'd for calls to upcalls.rust_personality. Used when
    // outputting the resume instruction.
    mut personality: Option<ValueRef>,
    // If this is a for-loop body that returns, this holds the pointers needed
    // for that
    mut loop_ret: Option<{flagptr: ValueRef, retptr: ValueRef}>,

    // Maps arguments to allocas created for them in llallocas.
    llargs: hashmap<ast::node_id, local_val>,
    // Maps the def_ids for local variables to the allocas created for
    // them in llallocas.
    lllocals: hashmap<ast::node_id, local_val>,
    // Same as above, but for closure upvars
    llupvars: hashmap<ast::node_id, ValueRef>,

    // The node_id of the function, or -1 if it doesn't correspond to
    // a user-defined function.
    id: ast::node_id,

    // If this function is being monomorphized, this contains the type
    // substitutions used.
    param_substs: Option<param_substs>,

    // The source span and nesting context where this function comes from, for
    // error reporting and symbol generation.
    span: Option<span>,
    path: path,

    // This function's enclosing crate context.
    ccx: @crate_ctxt
};

fn warn_not_to_commit(ccx: @crate_ctxt, msg: ~str) {
    if !ccx.do_not_commit_warning_issued {
        ccx.do_not_commit_warning_issued = true;
        ccx.sess.warn(msg + ~" -- do not commit like this!");
    }
}

// Heap selectors. Indicate which heap something should go on.
enum heap {
    heap_shared,
    heap_exchange,
}

enum cleantype {
    normal_exit_only,
    normal_exit_and_unwind
}

enum cleanup {
    clean(fn@(block) -> block, cleantype),
    clean_temp(ValueRef, fn@(block) -> block, cleantype),
}

// Used to remember and reuse existing cleanup paths
// target: none means the path ends in an resume instruction
type cleanup_path = {target: Option<BasicBlockRef>,
                     dest: BasicBlockRef};

fn scope_clean_changed(info: scope_info) {
    if info.cleanup_paths.len() > 0u { info.cleanup_paths = ~[]; }
    info.landing_pad = None;
}

fn cleanup_type(cx: ty::ctxt, ty: ty::t) -> cleantype {
    if ty::type_needs_unwind_cleanup(cx, ty) {
        normal_exit_and_unwind
    } else {
        normal_exit_only
    }
}

// This is not the same as base::root_value, which appears to be the vestigial
// remains of the previous GC regime. In the new GC, we can identify
// immediates on the stack without difficulty, but have trouble knowing where
// non-immediates are on the stack. For non-immediates, we must add an
// additional level of indirection, which allows us to alloca a pointer with
// the right addrspace.
fn root_for_cleanup(bcx: block, v: ValueRef, t: ty::t)
    -> {root: ValueRef, rooted: bool} {
    let ccx = bcx.ccx();

    let addrspace = base::get_tydesc(ccx, t).addrspace;
    if addrspace > gc_box_addrspace {
        let llty = type_of::type_of_rooted(ccx, t);
        let root = base::alloca(bcx, llty);
        build::Store(bcx, build::PointerCast(bcx, v, llty), root);
        {root: root, rooted: true}
    } else {
        {root: v, rooted: false}
    }
}

fn add_clean(bcx: block, val: ValueRef, t: ty::t) {
    if !ty::type_needs_drop(bcx.tcx(), t) { return; }
    debug!("add_clean(%s, %s, %s)",
           bcx.to_str(), val_str(bcx.ccx().tn, val),
           ty_to_str(bcx.ccx().tcx, t));
    let {root, rooted} = root_for_cleanup(bcx, val, t);
    let cleanup_type = cleanup_type(bcx.tcx(), t);
    do in_scope_cx(bcx) |info| {
        vec::push(info.cleanups,
                  clean(|a| base::drop_ty_root(a, root, rooted, t),
                        cleanup_type));
        scope_clean_changed(info);
    }
}
fn add_clean_temp_immediate(cx: block, val: ValueRef, ty: ty::t) {
    if !ty::type_needs_drop(cx.tcx(), ty) { return; }
    debug!("add_clean_temp_immediate(%s, %s, %s)",
           cx.to_str(), val_str(cx.ccx().tn, val),
           ty_to_str(cx.ccx().tcx, ty));
    let cleanup_type = cleanup_type(cx.tcx(), ty);
    do in_scope_cx(cx) |info| {
        vec::push(info.cleanups,
                  clean_temp(val, |a| base::drop_ty_immediate(a, val, ty),
                             cleanup_type));
        scope_clean_changed(info);
    }
}
fn add_clean_temp_mem(bcx: block, val: ValueRef, t: ty::t) {
    if !ty::type_needs_drop(bcx.tcx(), t) { return; }
    debug!("add_clean_temp_mem(%s, %s, %s)",
           bcx.to_str(), val_str(bcx.ccx().tn, val),
           ty_to_str(bcx.ccx().tcx, t));
    let {root, rooted} = root_for_cleanup(bcx, val, t);
    let cleanup_type = cleanup_type(bcx.tcx(), t);
    do in_scope_cx(bcx) |info| {
        vec::push(info.cleanups,
                  clean_temp(val, |a| base::drop_ty_root(a, root, rooted, t),
                             cleanup_type));
        scope_clean_changed(info);
    }
}
fn add_clean_free(cx: block, ptr: ValueRef, heap: heap) {
    let free_fn = match heap {
      heap_shared => |a| base::trans_free(a, ptr),
      heap_exchange => |a| base::trans_unique_free(a, ptr)
    };
    do in_scope_cx(cx) |info| {
        vec::push(info.cleanups, clean_temp(ptr, free_fn,
                                     normal_exit_and_unwind));
        scope_clean_changed(info);
    }
}

// Note that this only works for temporaries. We should, at some point, move
// to a system where we can also cancel the cleanup on local variables, but
// this will be more involved. For now, we simply zero out the local, and the
// drop glue checks whether it is zero.
fn revoke_clean(cx: block, val: ValueRef) {
    do in_scope_cx(cx) |info| {
        do option::iter(vec::position(info.cleanups, |cu| {
            match cu {
              clean_temp(v, _, _) if v == val => true,
              _ => false
            }
        })) |i| {
            info.cleanups =
                vec::append(vec::slice(info.cleanups, 0u, i),
                            vec::view(info.cleanups,
                                      i + 1u,
                                      info.cleanups.len()));
            scope_clean_changed(info);
        }
    }
}

fn block_cleanups(bcx: block) -> ~[cleanup] {
    match bcx.kind {
       block_non_scope  => ~[],
       block_scope(inf) => inf.cleanups
    }
}

enum block_kind {
    // A scope at the end of which temporary values created inside of it are
    // cleaned up. May correspond to an actual block in the language, but also
    // to an implicit scope, for example, calls introduce an implicit scope in
    // which the arguments are evaluated and cleaned up.
    block_scope(scope_info),
    // A non-scope block is a basic block created as a translation artifact
    // from translating code that expresses conditional logic rather than by
    // explicit { ... } block structure in the source language.  It's called a
    // non-scope block because it doesn't introduce a new variable scope.
    block_non_scope,
}

type scope_info = {
    loop_break: Option<block>,
    // A list of functions that must be run at when leaving this
    // block, cleaning up any variables that were introduced in the
    // block.
    mut cleanups: ~[cleanup],
    // Existing cleanup paths that may be reused, indexed by destination and
    // cleared when the set of cleanups changes.
    mut cleanup_paths: ~[cleanup_path],
    // Unwinding landing pad. Also cleared when cleanups change.
    mut landing_pad: Option<BasicBlockRef>,
};

trait get_node_info {
    fn info() -> Option<node_info>;
}

impl @ast::expr: get_node_info {
    fn info() -> Option<node_info> {
        Some({id: self.id, span: self.span})
    }
}

impl ast::blk: get_node_info {
    fn info() -> Option<node_info> {
        Some({id: self.node.id, span: self.span})
    }
}

// XXX: Work around a trait parsing bug. remove after snapshot
type optional_boxed_ast_expr = Option<@ast::expr>;

impl optional_boxed_ast_expr: get_node_info {
    fn info() -> Option<node_info> {
        self.chain(|s| s.info())
    }
}

type node_info = {
    id: ast::node_id,
    span: span
};

// Basic block context.  We create a block context for each basic block
// (single-entry, single-exit sequence of instructions) we generate from Rust
// code.  Each basic block we generate is attached to a function, typically
// with many basic blocks per function.  All the basic blocks attached to a
// function are organized as a directed graph.
struct block_ {
    // The BasicBlockRef returned from a call to
    // llvm::LLVMAppendBasicBlock(llfn, name), which adds a basic
    // block to the function pointed to by llfn.  We insert
    // instructions into that block by way of this block context.
    // The block pointing to this one in the function's digraph.
    let llbb: BasicBlockRef;
    let mut terminated: bool;
    let mut unreachable: bool;
    let parent: Option<block>;
    // The 'kind' of basic block this is.
    let kind: block_kind;
    // Is this block part of a landing pad?
    let is_lpad: bool;
    // info about the AST node this block originated from, if any
    let node_info: Option<node_info>;
    // The function context for the function to which this block is
    // attached.
    let fcx: fn_ctxt;
    new(llbb: BasicBlockRef, parent: Option<block>, -kind: block_kind,
        is_lpad: bool, node_info: Option<node_info>, fcx: fn_ctxt) {
        // sigh
        self.llbb = llbb; self.terminated = false; self.unreachable = false;
        self.parent = parent; self.kind = kind; self.is_lpad = is_lpad;
        self.node_info = node_info; self.fcx = fcx;
    }
}

/* This must be enum and not type, or trans goes into an infinite loop (#2572)
 */
enum block = @block_;

fn mk_block(llbb: BasicBlockRef, parent: Option<block>, -kind: block_kind,
            is_lpad: bool, node_info: Option<node_info>, fcx: fn_ctxt)
    -> block {
    block(@block_(llbb, parent, kind, is_lpad, node_info, fcx))
}

// First two args are retptr, env
const first_real_arg: uint = 2u;

type result = {bcx: block, val: ValueRef};
type result_t = {bcx: block, val: ValueRef, ty: ty::t};

fn rslt(bcx: block, val: ValueRef) -> result {
    {bcx: bcx, val: val}
}

fn ty_str(tn: type_names, t: TypeRef) -> ~str {
    return lib::llvm::type_to_str(tn, t);
}

fn val_ty(v: ValueRef) -> TypeRef { return llvm::LLVMTypeOf(v); }

fn val_str(tn: type_names, v: ValueRef) -> ~str {
    return ty_str(tn, val_ty(v));
}

// Returns the nth element of the given LLVM structure type.
fn struct_elt(llstructty: TypeRef, n: uint) -> TypeRef unsafe {
    let elt_count = llvm::LLVMCountStructElementTypes(llstructty) as uint;
    assert (n < elt_count);
    let elt_tys = vec::from_elem(elt_count, T_nil());
    llvm::LLVMGetStructElementTypes(llstructty, to_ptr(elt_tys));
    return llvm::LLVMGetElementType(elt_tys[n]);
}

fn in_scope_cx(cx: block, f: fn(scope_info)) {
    let mut cur = cx;
    loop {
        match cur.kind {
          block_scope(inf) => { f(inf); return; }
          _ => ()
        }
        cur = block_parent(cur);
    }
}

fn block_parent(cx: block) -> block {
    match cx.parent {
      Some(b) => b,
      None    => cx.sess().bug(fmt!("block_parent called on root block %?",
                                   cx))
    }
}

// Accessors

impl block {
    pure fn ccx() -> @crate_ctxt { self.fcx.ccx }
    pure fn tcx() -> ty::ctxt { self.fcx.ccx.tcx }
    pure fn sess() -> session { self.fcx.ccx.sess }

    fn val_str(val: ValueRef) -> ~str {
        val_str(self.ccx().tn, val)
    }
    fn ty_to_str(t: ty::t) -> ~str {
        ty_to_str(self.tcx(), t)
    }
    fn to_str() -> ~str {
        match self.node_info {
          Some(node_info) => {
            fmt!("[block %d]", node_info.id)
          }
          None => {
            fmt!("[block %x]", ptr::addr_of(*self) as uint)
          }
        }
    }
}

// LLVM type constructors.
fn T_void() -> TypeRef {
    // Note: For the time being llvm is kinda busted here, it has the notion
    // of a 'void' type that can only occur as part of the signature of a
    // function, but no general unit type of 0-sized value. This is, afaict,
    // vestigial from its C heritage, and we'll be attempting to submit a
    // patch upstream to fix it. In the mean time we only model function
    // outputs (Rust functions and C functions) using T_void, and model the
    // Rust general purpose nil type you can construct as 1-bit (always
    // zero). This makes the result incorrect for now -- things like a tuple
    // of 10 nil values will have 10-bit size -- but it doesn't seem like we
    // have any other options until it's fixed upstream.

    return llvm::LLVMVoidType();
}

fn T_nil() -> TypeRef {
    // NB: See above in T_void().

    return llvm::LLVMInt1Type();
}

fn T_metadata() -> TypeRef { return llvm::LLVMMetadataType(); }

fn T_i1() -> TypeRef { return llvm::LLVMInt1Type(); }

fn T_i8() -> TypeRef { return llvm::LLVMInt8Type(); }

fn T_i16() -> TypeRef { return llvm::LLVMInt16Type(); }

fn T_i32() -> TypeRef { return llvm::LLVMInt32Type(); }

fn T_i64() -> TypeRef { return llvm::LLVMInt64Type(); }

fn T_f32() -> TypeRef { return llvm::LLVMFloatType(); }

fn T_f64() -> TypeRef { return llvm::LLVMDoubleType(); }

fn T_bool() -> TypeRef { return T_i1(); }

fn T_int(targ_cfg: @session::config) -> TypeRef {
    return match targ_cfg.arch {
      session::arch_x86 => T_i32(),
      session::arch_x86_64 => T_i64(),
      session::arch_arm => T_i32()
    };
}

fn T_int_ty(cx: @crate_ctxt, t: ast::int_ty) -> TypeRef {
    match t {
      ast::ty_i => cx.int_type,
      ast::ty_char => T_char(),
      ast::ty_i8 => T_i8(),
      ast::ty_i16 => T_i16(),
      ast::ty_i32 => T_i32(),
      ast::ty_i64 => T_i64()
    }
}

fn T_uint_ty(cx: @crate_ctxt, t: ast::uint_ty) -> TypeRef {
    match t {
      ast::ty_u => cx.int_type,
      ast::ty_u8 => T_i8(),
      ast::ty_u16 => T_i16(),
      ast::ty_u32 => T_i32(),
      ast::ty_u64 => T_i64()
    }
}

fn T_float_ty(cx: @crate_ctxt, t: ast::float_ty) -> TypeRef {
    match t {
      ast::ty_f => cx.float_type,
      ast::ty_f32 => T_f32(),
      ast::ty_f64 => T_f64()
    }
}

fn T_float(targ_cfg: @session::config) -> TypeRef {
    return match targ_cfg.arch {
      session::arch_x86 => T_f64(),
      session::arch_x86_64 => T_f64(),
      session::arch_arm => T_f64()
    };
}

fn T_char() -> TypeRef { return T_i32(); }

fn T_size_t(targ_cfg: @session::config) -> TypeRef {
    return T_int(targ_cfg);
}

fn T_fn(inputs: ~[TypeRef], output: TypeRef) -> TypeRef unsafe {
    return llvm::LLVMFunctionType(output, to_ptr(inputs),
                               inputs.len() as c_uint,
                               False);
}

fn T_fn_pair(cx: @crate_ctxt, tfn: TypeRef) -> TypeRef {
    return T_struct(~[T_ptr(tfn), T_opaque_cbox_ptr(cx)]);
}

fn T_ptr(t: TypeRef) -> TypeRef {
    return llvm::LLVMPointerType(t, default_addrspace);
}

fn T_root(t: TypeRef, addrspace: addrspace) -> TypeRef {
    return llvm::LLVMPointerType(t, addrspace);
}

fn T_struct(elts: ~[TypeRef]) -> TypeRef unsafe {
    return llvm::LLVMStructType(to_ptr(elts), elts.len() as c_uint, False);
}

fn T_named_struct(name: ~str) -> TypeRef {
    let c = llvm::LLVMGetGlobalContext();
    return str::as_c_str(name, |buf| llvm::LLVMStructCreateNamed(c, buf));
}

fn set_struct_body(t: TypeRef, elts: ~[TypeRef]) unsafe {
    llvm::LLVMStructSetBody(t, to_ptr(elts),
                            elts.len() as c_uint, False);
}

fn T_empty_struct() -> TypeRef { return T_struct(~[]); }

// A vtable is, in reality, a vtable pointer followed by zero or more pointers
// to tydescs and other vtables that it closes over. But the types and number
// of those are rarely known to the code that needs to manipulate them, so
// they are described by this opaque type.
fn T_vtable() -> TypeRef { T_array(T_ptr(T_i8()), 1u) }

fn T_task(targ_cfg: @session::config) -> TypeRef {
    let t = T_named_struct(~"task");

    // Refcount
    // Delegate pointer
    // Stack segment pointer
    // Runtime SP
    // Rust SP
    // GC chain


    // Domain pointer
    // Crate cache pointer

    let t_int = T_int(targ_cfg);
    let elems =
        ~[t_int, t_int, t_int, t_int,
         t_int, t_int, t_int, t_int];
    set_struct_body(t, elems);
    return t;
}

fn T_tydesc_field(cx: @crate_ctxt, field: uint) -> TypeRef unsafe {
    // Bit of a kludge: pick the fn typeref out of the tydesc..

    let tydesc_elts: ~[TypeRef] =
        vec::from_elem::<TypeRef>(abi::n_tydesc_fields,
                                 T_nil());
    llvm::LLVMGetStructElementTypes(cx.tydesc_type,
                                    to_ptr::<TypeRef>(tydesc_elts));
    let t = llvm::LLVMGetElementType(tydesc_elts[field]);
    return t;
}

fn T_generic_glue_fn(cx: @crate_ctxt) -> TypeRef {
    let s = ~"glue_fn";
    match name_has_type(cx.tn, s) {
      Some(t) => return t,
      _ => ()
    }
    let t = T_tydesc_field(cx, abi::tydesc_field_drop_glue);
    associate_type(cx.tn, s, t);
    return t;
}

fn T_tydesc(targ_cfg: @session::config) -> TypeRef {
    let tydesc = T_named_struct(~"tydesc");
    let tydescpp = T_ptr(T_ptr(tydesc));
    let pvoid = T_ptr(T_i8());
    let glue_fn_ty =
        T_ptr(T_fn(~[T_ptr(T_nil()), T_ptr(T_nil()), tydescpp,
                    pvoid], T_void()));

    let int_type = T_int(targ_cfg);
    let elems =
        ~[int_type, int_type,
          glue_fn_ty, glue_fn_ty, glue_fn_ty, glue_fn_ty,
          T_ptr(T_i8()), T_ptr(T_i8())];
    set_struct_body(tydesc, elems);
    return tydesc;
}

fn T_array(t: TypeRef, n: uint) -> TypeRef {
    return llvm::LLVMArrayType(t, n as c_uint);
}

// Interior vector.
fn T_vec2(targ_cfg: @session::config, t: TypeRef) -> TypeRef {
    return T_struct(~[T_int(targ_cfg), // fill
                  T_int(targ_cfg), // alloc
                  T_array(t, 0u)]); // elements
}

fn T_vec(ccx: @crate_ctxt, t: TypeRef) -> TypeRef {
    return T_vec2(ccx.sess.targ_cfg, t);
}

// Note that the size of this one is in bytes.
fn T_opaque_vec(targ_cfg: @session::config) -> TypeRef {
    return T_vec2(targ_cfg, T_i8());
}

// Let T be the content of a box @T.  tuplify_box_ty(t) returns the
// representation of @T as a tuple (i.e., the ty::t version of what T_box()
// returns).
fn tuplify_box_ty(tcx: ty::ctxt, t: ty::t) -> ty::t {
    let ptr = ty::mk_ptr(tcx, {ty: ty::mk_nil(tcx), mutbl: ast::m_imm});
    return ty::mk_tup(tcx, ~[ty::mk_uint(tcx), ty::mk_type(tcx),
                         ptr, ptr,
                         t]);
}

fn T_box_header_fields(cx: @crate_ctxt) -> ~[TypeRef] {
    let ptr = T_ptr(T_i8());
    return ~[cx.int_type, T_ptr(cx.tydesc_type), ptr, ptr];
}

fn T_box_header(cx: @crate_ctxt) -> TypeRef {
    return T_struct(T_box_header_fields(cx));
}

fn T_box(cx: @crate_ctxt, t: TypeRef) -> TypeRef {
    return T_struct(vec::append(T_box_header_fields(cx), ~[t]));
}

fn T_box_ptr(t: TypeRef) -> TypeRef {
    return llvm::LLVMPointerType(t, gc_box_addrspace);
}

fn T_opaque_box(cx: @crate_ctxt) -> TypeRef {
    return T_box(cx, T_i8());
}

fn T_opaque_box_ptr(cx: @crate_ctxt) -> TypeRef {
    return T_box_ptr(T_opaque_box(cx));
}

fn T_unique(cx: @crate_ctxt, t: TypeRef) -> TypeRef {
    return T_struct(vec::append(T_box_header_fields(cx), ~[t]));
}

fn T_unique_ptr(t: TypeRef) -> TypeRef {
    return llvm::LLVMPointerType(t, gc_box_addrspace);
}

fn T_port(cx: @crate_ctxt, _t: TypeRef) -> TypeRef {
    return T_struct(~[cx.int_type]); // Refcount

}

fn T_chan(cx: @crate_ctxt, _t: TypeRef) -> TypeRef {
    return T_struct(~[cx.int_type]); // Refcount

}

fn T_taskptr(cx: @crate_ctxt) -> TypeRef { return T_ptr(cx.task_type); }


// This type must never be used directly; it must always be cast away.
fn T_typaram(tn: type_names) -> TypeRef {
    let s = ~"typaram";
    match name_has_type(tn, s) {
      Some(t) => return t,
      _ => ()
    }
    let t = T_i8();
    associate_type(tn, s, t);
    return t;
}

fn T_typaram_ptr(tn: type_names) -> TypeRef { return T_ptr(T_typaram(tn)); }

fn T_opaque_cbox_ptr(cx: @crate_ctxt) -> TypeRef {
    // closures look like boxes (even when they are fn~ or fn&)
    // see trans_closure.rs
    return T_opaque_box_ptr(cx);
}

fn T_enum_discrim(cx: @crate_ctxt) -> TypeRef {
    return cx.int_type;
}

fn T_opaque_enum(cx: @crate_ctxt) -> TypeRef {
    let s = ~"opaque_enum";
    match name_has_type(cx.tn, s) {
      Some(t) => return t,
      _ => ()
    }
    let t = T_struct(~[T_enum_discrim(cx), T_i8()]);
    associate_type(cx.tn, s, t);
    return t;
}

fn T_opaque_enum_ptr(cx: @crate_ctxt) -> TypeRef {
    return T_ptr(T_opaque_enum(cx));
}

fn T_captured_tydescs(cx: @crate_ctxt, n: uint) -> TypeRef {
    return T_struct(vec::from_elem::<TypeRef>(n, T_ptr(cx.tydesc_type)));
}

fn T_opaque_trait(cx: @crate_ctxt) -> TypeRef {
    T_struct(~[T_ptr(cx.tydesc_type), T_opaque_box_ptr(cx)])
}

fn T_opaque_port_ptr() -> TypeRef { return T_ptr(T_i8()); }

fn T_opaque_chan_ptr() -> TypeRef { return T_ptr(T_i8()); }


// LLVM constant constructors.
fn C_null(t: TypeRef) -> ValueRef { return llvm::LLVMConstNull(t); }

fn C_integral(t: TypeRef, u: u64, sign_extend: Bool) -> ValueRef {
    return llvm::LLVMConstInt(t, u, sign_extend);
}

fn C_floating(s: ~str, t: TypeRef) -> ValueRef {
    return str::as_c_str(s, |buf| llvm::LLVMConstRealOfString(t, buf));
}

fn C_nil() -> ValueRef {
    // NB: See comment above in T_void().

    return C_integral(T_i1(), 0u64, False);
}

fn C_bool(b: bool) -> ValueRef {
    C_integral(T_bool(), if b { 1u64 } else { 0u64 }, False)
}

fn C_i32(i: i32) -> ValueRef {
    return C_integral(T_i32(), i as u64, True);
}

fn C_i64(i: i64) -> ValueRef {
    return C_integral(T_i64(), i as u64, True);
}

fn C_int(cx: @crate_ctxt, i: int) -> ValueRef {
    return C_integral(cx.int_type, i as u64, True);
}

fn C_uint(cx: @crate_ctxt, i: uint) -> ValueRef {
    return C_integral(cx.int_type, i as u64, False);
}

fn C_u8(i: uint) -> ValueRef { return C_integral(T_i8(), i as u64, False); }


// This is a 'c-like' raw string, which differs from
// our boxed-and-length-annotated strings.
fn C_cstr(cx: @crate_ctxt, s: ~str) -> ValueRef {
    match cx.const_cstr_cache.find(s) {
      Some(llval) => return llval,
      None => ()
    }

    let sc = do str::as_c_str(s) |buf| {
        llvm::LLVMConstString(buf, str::len(s) as c_uint, False)
    };
    let g =
        str::as_c_str(fmt!("str%u", cx.names(~"str")),
                    |buf| llvm::LLVMAddGlobal(cx.llmod, val_ty(sc), buf));
    llvm::LLVMSetInitializer(g, sc);
    llvm::LLVMSetGlobalConstant(g, True);
    lib::llvm::SetLinkage(g, lib::llvm::InternalLinkage);

    cx.const_cstr_cache.insert(s, g);

    return g;
}

fn C_estr_slice(cx: @crate_ctxt, s: ~str) -> ValueRef {
    let cs = llvm::LLVMConstPointerCast(C_cstr(cx, s), T_ptr(T_i8()));
    C_struct(~[cs, C_uint(cx, str::len(s) + 1u /* +1 for null */)])
}

// Returns a Plain Old LLVM String:
fn C_postr(s: ~str) -> ValueRef {
    return do str::as_c_str(s) |buf| {
        llvm::LLVMConstString(buf, str::len(s) as c_uint, False)
    };
}

fn C_zero_byte_arr(size: uint) -> ValueRef unsafe {
    let mut i = 0u;
    let mut elts: ~[ValueRef] = ~[];
    while i < size { vec::push(elts, C_u8(0u)); i += 1u; }
    return llvm::LLVMConstArray(T_i8(), vec::unsafe::to_ptr(elts),
                             elts.len() as c_uint);
}

fn C_struct(elts: ~[ValueRef]) -> ValueRef unsafe {
    return llvm::LLVMConstStruct(vec::unsafe::to_ptr(elts),
                              elts.len() as c_uint, False);
}

fn C_named_struct(T: TypeRef, elts: ~[ValueRef]) -> ValueRef unsafe {
    return llvm::LLVMConstNamedStruct(T, vec::unsafe::to_ptr(elts),
                                   elts.len() as c_uint);
}

fn C_array(ty: TypeRef, elts: ~[ValueRef]) -> ValueRef unsafe {
    return llvm::LLVMConstArray(ty, vec::unsafe::to_ptr(elts),
                             elts.len() as c_uint);
}

fn C_bytes(bytes: ~[u8]) -> ValueRef unsafe {
    return llvm::LLVMConstString(
        unsafe::reinterpret_cast(&vec::unsafe::to_ptr(bytes)),
        bytes.len() as c_uint, True);
}

fn C_bytes_plus_null(bytes: ~[u8]) -> ValueRef unsafe {
    return llvm::LLVMConstString(
        unsafe::reinterpret_cast(&vec::unsafe::to_ptr(bytes)),
        bytes.len() as c_uint, False);
}

fn C_shape(ccx: @crate_ctxt, bytes: ~[u8]) -> ValueRef {
    let llshape = C_bytes_plus_null(bytes);
    let llglobal = str::as_c_str(fmt!("shape%u", ccx.names(~"shape")), |buf| {
        llvm::LLVMAddGlobal(ccx.llmod, val_ty(llshape), buf)
    });
    llvm::LLVMSetInitializer(llglobal, llshape);
    llvm::LLVMSetGlobalConstant(llglobal, True);
    lib::llvm::SetLinkage(llglobal, lib::llvm::InternalLinkage);
    return llvm::LLVMConstPointerCast(llglobal, T_ptr(T_i8()));
}

fn get_param(fndecl: ValueRef, param: uint) -> ValueRef {
    llvm::LLVMGetParam(fndecl, param as c_uint)
}

// Used to identify cached monomorphized functions and vtables
enum mono_param_id {
    mono_precise(ty::t, Option<~[mono_id]>),
    mono_any,
    mono_repr(uint /* size */, uint /* align */),
}

type mono_id_ = {def: ast::def_id, params: ~[mono_param_id]};

type mono_id = @mono_id_;

impl mono_param_id: cmp::Eq {
    pure fn eq(&&other: mono_param_id) -> bool {
        match (self, other) {
            (mono_precise(ty_a, ids_a), mono_precise(ty_b, ids_b)) => {
                ty_a == ty_b && ids_a == ids_b
            }
            (mono_any, mono_any) => true,
            (mono_repr(size_a, align_a), mono_repr(size_b, align_b)) => {
                size_a == size_b && align_a == align_b
            }
            (mono_precise(*), _) => false,
            (mono_any, _) => false,
            (mono_repr(*), _) => false
        }
    }
}

impl mono_id_: cmp::Eq {
    pure fn eq(&&other: mono_id_) -> bool {
        return self.def == other.def && self.params == other.params;
    }
}

pure fn hash_mono_id(mi: &mono_id) -> uint {
    let mut h = syntax::ast_util::hash_def(&mi.def);
    for vec::each(mi.params) |param| {
        h = h * match param {
          mono_precise(ty, vts) => {
            let mut h = ty::type_id(ty);
            do option::iter(vts) |vts| {
                for vec::each(vts) |vt| {
                    h += hash_mono_id(&vt);
                }
            }
            h
          }
          mono_any => 1u,
          mono_repr(sz, align) => sz * (align + 2u)
        }
    }
    h
}

fn umax(cx: block, a: ValueRef, b: ValueRef) -> ValueRef {
    let cond = build::ICmp(cx, lib::llvm::IntULT, a, b);
    return build::Select(cx, cond, b, a);
}

fn umin(cx: block, a: ValueRef, b: ValueRef) -> ValueRef {
    let cond = build::ICmp(cx, lib::llvm::IntULT, a, b);
    return build::Select(cx, cond, a, b);
}

fn align_to(cx: block, off: ValueRef, align: ValueRef) -> ValueRef {
    let mask = build::Sub(cx, align, C_int(cx.ccx(), 1));
    let bumped = build::Add(cx, off, mask);
    return build::And(cx, bumped, build::Not(cx, mask));
}

fn path_str(sess: session::session, p: path) -> ~str {
    let mut r = ~"", first = true;
    for vec::each(p) |e| {
        match e { ast_map::path_name(s) | ast_map::path_mod(s) => {
          if first { first = false; }
          else { r += ~"::"; }
          r += sess.str_of(s);
        } }
    }
    r
}

fn node_id_type(bcx: block, id: ast::node_id) -> ty::t {
    let tcx = bcx.tcx();
    let t = ty::node_id_to_type(tcx, id);
    match bcx.fcx.param_substs {
      Some(substs) => ty::subst_tps(tcx, substs.tys, t),
      _ => { assert !ty::type_has_params(t); t }
    }
}
fn expr_ty(bcx: block, ex: @ast::expr) -> ty::t {
    node_id_type(bcx, ex.id)
}
fn node_id_type_params(bcx: block, id: ast::node_id) -> ~[ty::t] {
    let tcx = bcx.tcx();
    let params = ty::node_id_to_type_params(tcx, id);
    match bcx.fcx.param_substs {
      Some(substs) => {
        vec::map(params, |t| ty::subst_tps(tcx, substs.tys, t))
      }
      _ => params
    }
}

fn field_idx_strict(cx: ty::ctxt, sp: span, ident: ast::ident,
                    fields: ~[ty::field])
    -> uint {
    match ty::field_idx(ident, fields) {
       None => cx.sess.span_bug(
           sp, fmt!("base expr doesn't appear to \
                         have a field named %s", cx.sess.str_of(ident))),
       Some(i) => i
    }
}

fn dummy_substs(tps: ~[ty::t]) -> ty::substs {
    {self_r: Some(ty::re_bound(ty::br_self)),
     self_ty: None,
     tps: tps}
}

impl cleantype : cmp::Eq {
    pure fn eq(&&other: cleantype) -> bool {
        match self {
            normal_exit_only => {
                match other {
                    normal_exit_only => true,
                    _ => false
                }
            }
            normal_exit_and_unwind => {
                match other {
                    normal_exit_and_unwind => true,
                    _ => false
                }
            }
        }
    }
}

//
// Local Variables:
// mode: rust
// fill-column: 78;
// indent-tabs-mode: nil
// c-basic-offset: 4
// buffer-file-coding-system: utf-8-unix
// End:
//
