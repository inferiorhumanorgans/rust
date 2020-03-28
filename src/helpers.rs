use std::convert::TryFrom;
use std::mem;

use rustc::mir;
use rustc::ty::{
    self,
    layout::{self, LayoutOf, Size, TyLayout},
    List, TyCtxt,
};
use rustc_hir::def_id::{DefId, CRATE_DEF_INDEX};
use rustc_span::source_map::DUMMY_SP;

use rand::RngCore;

use crate::*;

impl<'mir, 'tcx> EvalContextExt<'mir, 'tcx> for crate::MiriEvalContext<'mir, 'tcx> {}

/// Gets an instance for a path.
fn try_resolve_did<'mir, 'tcx>(tcx: TyCtxt<'tcx>, path: &[&str]) -> Option<DefId> {
    tcx.crates()
        .iter()
        .find(|&&krate| tcx.original_crate_name(krate).as_str() == path[0])
        .and_then(|krate| {
            let krate = DefId { krate: *krate, index: CRATE_DEF_INDEX };
            let mut items = tcx.item_children(krate);
            let mut path_it = path.iter().skip(1).peekable();

            while let Some(segment) = path_it.next() {
                for item in mem::replace(&mut items, Default::default()).iter() {
                    if item.ident.name.as_str() == *segment {
                        if path_it.peek().is_none() {
                            return Some(item.res.def_id());
                        }

                        items = tcx.item_children(item.res.def_id());
                        break;
                    }
                }
            }
            None
        })
}

pub trait EvalContextExt<'mir, 'tcx: 'mir>: crate::MiriEvalContextExt<'mir, 'tcx> {
    /// Gets an instance for a path.
    fn resolve_path(&self, path: &[&str]) -> ty::Instance<'tcx> {
        let did = try_resolve_did(self.eval_context_ref().tcx.tcx, path)
            .unwrap_or_else(|| panic!("failed to find required Rust item: {:?}", path));
        ty::Instance::mono(self.eval_context_ref().tcx.tcx, did)
    }

    /// Evaluates the scalar at the specified path. Returns Some(val)
    /// if the path could be resolved, and None otherwise
    fn eval_path_scalar(
        &mut self,
        path: &[&str],
    ) -> InterpResult<'tcx, ScalarMaybeUndef<Tag>> {
        let this = self.eval_context_mut();
        let instance = this.resolve_path(path);
        let cid = GlobalId { instance, promoted: None };
        let const_val = this.const_eval_raw(cid)?;
        let const_val = this.read_scalar(const_val.into())?;
        return Ok(const_val);
    }

    /// Helper function to get a `libc` constant as a `Scalar`.
    fn eval_libc(&mut self, name: &str) -> InterpResult<'tcx, Scalar<Tag>> {
        self.eval_context_mut()
            .eval_path_scalar(&["libc", name])?
            .not_undef()
    }

    /// Helper function to get a `windows` constant as a `Scalar`.
    fn eval_windows(&mut self, name: &str) -> InterpResult<'tcx, Scalar<Tag>> {
        self.eval_context_mut()
            .eval_path_scalar(&["std", "sys", "windows", "c", name])?
            .not_undef()
    }

    /// Helper function to get a `libc` constant as an `i32`.
    fn eval_libc_i32(&mut self, name: &str) -> InterpResult<'tcx, i32> {
        // TODO: Cache the result.
        self.eval_libc(name)?.to_i32()
    }

    /// Helper function to get the `TyLayout` of a `libc` type
    fn libc_ty_layout(&mut self, name: &str) -> InterpResult<'tcx, TyLayout<'tcx>> {
        let this = self.eval_context_mut();
        let ty = this.resolve_path(&["libc", name]).monomorphic_ty(*this.tcx);
        this.layout_of(ty)
    }

    /// Write a 0 of the appropriate size to `dest`.
    fn write_null(&mut self, dest: PlaceTy<'tcx, Tag>) -> InterpResult<'tcx> {
        self.eval_context_mut().write_scalar(Scalar::from_int(0, dest.layout.size), dest)
    }

    /// Test if this immediate equals 0.
    fn is_null(&self, val: Scalar<Tag>) -> InterpResult<'tcx, bool> {
        let this = self.eval_context_ref();
        let null = Scalar::null_ptr(this);
        this.ptr_eq(val, null)
    }

    /// Turn a Scalar into an Option<NonNullScalar>
    fn test_null(&self, val: Scalar<Tag>) -> InterpResult<'tcx, Option<Scalar<Tag>>> {
        let this = self.eval_context_ref();
        Ok(if this.is_null(val)? { None } else { Some(val) })
    }

    /// Get the `Place` for a local
    fn local_place(&mut self, local: mir::Local) -> InterpResult<'tcx, PlaceTy<'tcx, Tag>> {
        let this = self.eval_context_mut();
        let place = mir::Place { local: local, projection: List::empty() };
        this.eval_place(&place)
    }

    /// Generate some random bytes, and write them to `dest`.
    fn gen_random(&mut self, ptr: Scalar<Tag>, len: u64) -> InterpResult<'tcx> {
        // Some programs pass in a null pointer and a length of 0
        // to their platform's random-generation function (e.g. getrandom())
        // on Linux. For compatibility with these programs, we don't perform
        // any additional checks - it's okay if the pointer is invalid,
        // since we wouldn't actually be writing to it.
        if len == 0 {
            return Ok(());
        }
        let this = self.eval_context_mut();

        let mut data = vec![0; usize::try_from(len).unwrap()];

        if this.machine.communicate {
            // Fill the buffer using the host's rng.
            getrandom::getrandom(&mut data)
                .map_err(|err| err_unsup_format!("host getrandom failed: {}", err))?;
        } else {
            let rng = this.memory.extra.rng.get_mut();
            rng.fill_bytes(&mut data);
        }

        this.memory.write_bytes(ptr, data.iter().copied())
    }

    /// Call a function: Push the stack frame and pass the arguments.
    /// For now, arguments must be scalars (so that the caller does not have to know the layout).
    fn call_function(
        &mut self,
        f: ty::Instance<'tcx>,
        args: &[Immediate<Tag>],
        dest: Option<PlaceTy<'tcx, Tag>>,
        stack_pop: StackPopCleanup,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        // Push frame.
        let mir = &*this.load_mir(f.def, None)?;
        let span = this
            .stack()
            .last()
            .and_then(Frame::current_source_info)
            .map(|si| si.span)
            .unwrap_or(DUMMY_SP);
        this.push_stack_frame(f, span, mir, dest, stack_pop)?;

        // Initialize arguments.
        let mut callee_args = this.frame().body.args_iter();
        for arg in args {
            let callee_arg = this.local_place(
                callee_args.next().expect("callee has fewer arguments than expected"),
            )?;
            this.write_immediate(*arg, callee_arg)?;
        }
        callee_args.next().expect_none("callee has more arguments than expected");

        Ok(())
    }

    /// Visits the memory covered by `place`, sensitive to freezing: the 3rd parameter
    /// will be true if this is frozen, false if this is in an `UnsafeCell`.
    fn visit_freeze_sensitive(
        &self,
        place: MPlaceTy<'tcx, Tag>,
        size: Size,
        mut action: impl FnMut(Pointer<Tag>, Size, bool) -> InterpResult<'tcx>,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_ref();
        trace!("visit_frozen(place={:?}, size={:?})", *place, size);
        debug_assert_eq!(
            size,
            this.size_and_align_of_mplace(place)?
                .map(|(size, _)| size)
                .unwrap_or_else(|| place.layout.size)
        );
        // Store how far we proceeded into the place so far. Everything to the left of
        // this offset has already been handled, in the sense that the frozen parts
        // have had `action` called on them.
        let mut end_ptr = place.ptr.assert_ptr();
        // Called when we detected an `UnsafeCell` at the given offset and size.
        // Calls `action` and advances `end_ptr`.
        let mut unsafe_cell_action = |unsafe_cell_ptr: Scalar<Tag>, unsafe_cell_size: Size| {
            let unsafe_cell_ptr = unsafe_cell_ptr.assert_ptr();
            debug_assert_eq!(unsafe_cell_ptr.alloc_id, end_ptr.alloc_id);
            debug_assert_eq!(unsafe_cell_ptr.tag, end_ptr.tag);
            // We assume that we are given the fields in increasing offset order,
            // and nothing else changes.
            let unsafe_cell_offset = unsafe_cell_ptr.offset;
            let end_offset = end_ptr.offset;
            assert!(unsafe_cell_offset >= end_offset);
            let frozen_size = unsafe_cell_offset - end_offset;
            // Everything between the end_ptr and this `UnsafeCell` is frozen.
            if frozen_size != Size::ZERO {
                action(end_ptr, frozen_size, /*frozen*/ true)?;
            }
            // This `UnsafeCell` is NOT frozen.
            if unsafe_cell_size != Size::ZERO {
                action(unsafe_cell_ptr, unsafe_cell_size, /*frozen*/ false)?;
            }
            // Update end end_ptr.
            end_ptr = unsafe_cell_ptr.wrapping_offset(unsafe_cell_size, this);
            // Done
            Ok(())
        };
        // Run a visitor
        {
            let mut visitor = UnsafeCellVisitor {
                ecx: this,
                unsafe_cell_action: |place| {
                    trace!("unsafe_cell_action on {:?}", place.ptr);
                    // We need a size to go on.
                    let unsafe_cell_size = this
                        .size_and_align_of_mplace(place)?
                        .map(|(size, _)| size)
                        // for extern types, just cover what we can
                        .unwrap_or_else(|| place.layout.size);
                    // Now handle this `UnsafeCell`, unless it is empty.
                    if unsafe_cell_size != Size::ZERO {
                        unsafe_cell_action(place.ptr, unsafe_cell_size)
                    } else {
                        Ok(())
                    }
                },
            };
            visitor.visit_value(place)?;
        }
        // The part between the end_ptr and the end of the place is also frozen.
        // So pretend there is a 0-sized `UnsafeCell` at the end.
        unsafe_cell_action(place.ptr.ptr_wrapping_offset(size, this), Size::ZERO)?;
        // Done!
        return Ok(());

        /// Visiting the memory covered by a `MemPlace`, being aware of
        /// whether we are inside an `UnsafeCell` or not.
        struct UnsafeCellVisitor<'ecx, 'mir, 'tcx, F>
        where
            F: FnMut(MPlaceTy<'tcx, Tag>) -> InterpResult<'tcx>,
        {
            ecx: &'ecx MiriEvalContext<'mir, 'tcx>,
            unsafe_cell_action: F,
        }

        impl<'ecx, 'mir, 'tcx, F> ValueVisitor<'mir, 'tcx, Evaluator<'tcx>>
            for UnsafeCellVisitor<'ecx, 'mir, 'tcx, F>
        where
            F: FnMut(MPlaceTy<'tcx, Tag>) -> InterpResult<'tcx>,
        {
            type V = MPlaceTy<'tcx, Tag>;

            #[inline(always)]
            fn ecx(&self) -> &MiriEvalContext<'mir, 'tcx> {
                &self.ecx
            }

            // Hook to detect `UnsafeCell`.
            fn visit_value(&mut self, v: MPlaceTy<'tcx, Tag>) -> InterpResult<'tcx> {
                trace!("UnsafeCellVisitor: {:?} {:?}", *v, v.layout.ty);
                let is_unsafe_cell = match v.layout.ty.kind {
                    ty::Adt(adt, _) =>
                        Some(adt.did) == self.ecx.tcx.lang_items().unsafe_cell_type(),
                    _ => false,
                };
                if is_unsafe_cell {
                    // We do not have to recurse further, this is an `UnsafeCell`.
                    (self.unsafe_cell_action)(v)
                } else if self.ecx.type_is_freeze(v.layout.ty) {
                    // This is `Freeze`, there cannot be an `UnsafeCell`
                    Ok(())
                } else {
                    // We want to not actually read from memory for this visit. So, before
                    // walking this value, we have to make sure it is not a
                    // `Variants::Multiple`.
                    match v.layout.variants {
                        layout::Variants::Multiple { .. } => {
                            // A multi-variant enum, or generator, or so.
                            // Treat this like a union: without reading from memory,
                            // we cannot determine the variant we are in. Reading from
                            // memory would be subject to Stacked Borrows rules, leading
                            // to all sorts of "funny" recursion.
                            // We only end up here if the type is *not* freeze, so we just call the
                            // `UnsafeCell` action.
                            (self.unsafe_cell_action)(v)
                        }
                        layout::Variants::Single { .. } => {
                            // Proceed further, try to find where exactly that `UnsafeCell`
                            // is hiding.
                            self.walk_value(v)
                        }
                    }
                }
            }

            // Make sure we visit aggregrates in increasing offset order.
            fn visit_aggregate(
                &mut self,
                place: MPlaceTy<'tcx, Tag>,
                fields: impl Iterator<Item = InterpResult<'tcx, MPlaceTy<'tcx, Tag>>>,
            ) -> InterpResult<'tcx> {
                match place.layout.fields {
                    layout::FieldPlacement::Array { .. } => {
                        // For the array layout, we know the iterator will yield sorted elements so
                        // we can avoid the allocation.
                        self.walk_aggregate(place, fields)
                    }
                    layout::FieldPlacement::Arbitrary { .. } => {
                        // Gather the subplaces and sort them before visiting.
                        let mut places =
                            fields.collect::<InterpResult<'tcx, Vec<MPlaceTy<'tcx, Tag>>>>()?;
                        places.sort_by_key(|place| place.ptr.assert_ptr().offset);
                        self.walk_aggregate(place, places.into_iter().map(Ok))
                    }
                    layout::FieldPlacement::Union { .. } => {
                        // Uh, what?
                        bug!("a union is not an aggregate we should ever visit")
                    }
                }
            }

            // We have to do *something* for unions.
            fn visit_union(&mut self, v: MPlaceTy<'tcx, Tag>, fields: usize) -> InterpResult<'tcx> {
                assert!(fields > 0); // we should never reach "pseudo-unions" with 0 fields, like primitives

                // With unions, we fall back to whatever the type says, to hopefully be consistent
                // with LLVM IR.
                // FIXME: are we consistent, and is this really the behavior we want?
                let frozen = self.ecx.type_is_freeze(v.layout.ty);
                if frozen { Ok(()) } else { (self.unsafe_cell_action)(v) }
            }
        }
    }

    // Writes several `ImmTy`s contiguosly into memory. This is useful when you have to pack
    // different values into a struct.
    fn write_packed_immediates(
        &mut self,
        place: MPlaceTy<'tcx, Tag>,
        imms: &[ImmTy<'tcx, Tag>],
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        let mut offset = Size::from_bytes(0);

        for &imm in imms {
            this.write_immediate_to_mplace(
                *imm,
                place.offset(offset, MemPlaceMeta::None, imm.layout, &*this.tcx)?,
            )?;
            offset += imm.layout.size;
        }
        Ok(())
    }

    /// Helper function used inside the shims of foreign functions to check that isolation is
    /// disabled. It returns an error using the `name` of the foreign function if this is not the
    /// case.
    fn check_no_isolation(&self, name: &str) -> InterpResult<'tcx> {
        if !self.eval_context_ref().machine.communicate {
            throw_machine_stop!(TerminationInfo::UnsupportedInIsolation(format!(
                "`{}` not available when isolation is enabled",
                name,
            )))
        }
        Ok(())
    }
    /// Helper function used inside the shims of foreign functions to assert that the target OS
    /// is `target_os`. It panics showing a message with the `name` of the foreign function
    /// if this is not the case.
    fn assert_target_os(&self, target_os: &str, name: &str) {
        assert_eq!(
            self.eval_context_ref().tcx.sess.target.target.target_os,
            target_os,
            "`{}` is only available on the `{}` target OS",
            name,
            target_os,
        )
    }

    /// Sets the last error variable.
    fn set_last_error(&mut self, scalar: Scalar<Tag>) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        let errno_place = this.machine.last_error.unwrap();
        this.write_scalar(scalar, errno_place.into())
    }

    /// Gets the last error variable.
    fn get_last_error(&self) -> InterpResult<'tcx, Scalar<Tag>> {
        let this = self.eval_context_ref();
        let errno_place = this.machine.last_error.unwrap();
        this.read_scalar(errno_place.into())?.not_undef()
    }

    /// Sets the last OS error using a `std::io::Error`. This function tries to produce the most
    /// similar OS error from the `std::io::ErrorKind` and sets it as the last OS error.
    fn set_last_error_from_io_error(&mut self, e: std::io::Error) -> InterpResult<'tcx> {
        use std::io::ErrorKind::*;
        let this = self.eval_context_mut();
        let target = &this.tcx.sess.target.target;
        let last_error = if target.options.target_family == Some("unix".to_owned()) {
            this.eval_libc(match e.kind() {
                ConnectionRefused => "ECONNREFUSED",
                ConnectionReset => "ECONNRESET",
                PermissionDenied => "EPERM",
                BrokenPipe => "EPIPE",
                NotConnected => "ENOTCONN",
                ConnectionAborted => "ECONNABORTED",
                AddrNotAvailable => "EADDRNOTAVAIL",
                AddrInUse => "EADDRINUSE",
                NotFound => "ENOENT",
                Interrupted => "EINTR",
                InvalidInput => "EINVAL",
                TimedOut => "ETIMEDOUT",
                AlreadyExists => "EEXIST",
                WouldBlock => "EWOULDBLOCK",
                _ => {
                    throw_unsup_format!("io error {} cannot be transformed into a raw os error", e)
                }
            })?
        } else {
            // FIXME: we have to finish implementing the Windows equivalent of this.
            this.eval_windows(match e.kind() {
                NotFound => "ERROR_FILE_NOT_FOUND",
                _ => throw_unsup_format!(
                    "setting the last OS error from an io::Error is yet unsupported for {}.",
                    target.target_os
                )
            })?
        };
        this.set_last_error(last_error)
    }

    /// Helper function that consumes an `std::io::Result<T>` and returns an
    /// `InterpResult<'tcx,T>::Ok` instead. In case the result is an error, this function returns
    /// `Ok(-1)` and sets the last OS error accordingly.
    ///
    /// This function uses `T: From<i32>` instead of `i32` directly because some IO related
    /// functions return different integer types (like `read`, that returns an `i64`).
    fn try_unwrap_io_result<T: From<i32>>(
        &mut self,
        result: std::io::Result<T>,
    ) -> InterpResult<'tcx, T> {
        match result {
            Ok(ok) => Ok(ok),
            Err(e) => {
                self.eval_context_mut().set_last_error_from_io_error(e)?;
                Ok((-1).into())
            }
        }
    }
}

pub fn immty_from_int_checked<'tcx>(
    int: impl Into<i128>,
    layout: TyLayout<'tcx>,
) -> InterpResult<'tcx, ImmTy<'tcx, Tag>> {
    let int = int.into();
    Ok(ImmTy::try_from_int(int, layout).ok_or_else(|| {
        err_unsup_format!("signed value {:#x} does not fit in {} bits", int, layout.size.bits())
    })?)
}

pub fn immty_from_uint_checked<'tcx>(
    int: impl Into<u128>,
    layout: TyLayout<'tcx>,
) -> InterpResult<'tcx, ImmTy<'tcx, Tag>> {
    let int = int.into();
    Ok(ImmTy::try_from_uint(int, layout).ok_or_else(|| {
        err_unsup_format!("unsigned value {:#x} does not fit in {} bits", int, layout.size.bits())
    })?)
}
