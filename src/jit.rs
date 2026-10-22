//! Native control flow and arithmetic generated with Cranelift. The owning
//! module stays alive for every invocation; function pointers never escape it.
use crate::{
    ir::{Expr, Op, Program},
    runtime::*,
};
use anyhow::{Context, Result};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use std::collections::HashMap;
use std::mem::offset_of;

pub(crate) struct NativeProgram {
    module: Option<JITModule>,
    entry: cranelift_module::FuncId,
}
impl NativeProgram {
    pub fn compile(program: &Program) -> Result<Self> {
        let mut jit = JITBuilder::with_flags(&[("opt_level", "speed")], default_libcall_names())?;
        let imports: Vec<(&str, *const u8, Vec<ir::Type>, Option<ir::Type>)> = vec![
            (
                "list",
                rt_list as *const u8,
                vec![types::I32, types::I32, types::I64, types::I64],
                Some(types::I64),
            ),
            (
                "checkpoint",
                rt_checkpoint as *const u8,
                vec![types::I32],
                Some(types::I32),
            ),
            (
                "number",
                rt_number as *const u8,
                vec![types::I64],
                Some(types::F64),
            ),
            (
                "bool",
                rt_bool as *const u8,
                vec![types::I64],
                Some(types::I32),
            ),
            (
                "call_args",
                rt_call_args as *const u8,
                vec![types::I32],
                Some(if usize::BITS == 64 {
                    types::I64
                } else {
                    types::I32
                }),
            ),
            (
                "call",
                rt_call as *const u8,
                vec![types::I32, types::I32],
                Some(types::I32),
            ),
            ("return", rt_return as *const u8, vec![], Some(types::I32)),
            (
                "loop_set",
                rt_loop_set as *const u8,
                vec![types::I32, types::F64],
                None,
            ),
            (
                "loop_get",
                rt_loop_get as *const u8,
                vec![types::I32],
                Some(types::F64),
            ),
            ("loop_dec", rt_loop_dec as *const u8, vec![types::I32], None),
            (
                "wait",
                rt_wait as *const u8,
                vec![types::F64, types::I32],
                None,
            ),
            (
                "broadcast",
                rt_broadcast as *const u8,
                vec![types::I64, types::I32, types::I32],
                None,
            ),
            ("stop", rt_stop as *const u8, vec![types::I32], None),
            (
                "binary",
                rt_binary as *const u8,
                vec![types::I32, types::I64, types::I64],
                Some(types::I64),
            ),
            (
                "unary",
                rt_unary as *const u8,
                vec![types::I32, types::I64],
                Some(types::I64),
            ),
            (
                "state",
                rt_state as *const u8,
                vec![types::I32],
                Some(types::I64),
            ),
            (
                "action",
                rt_action as *const u8,
                vec![types::I32, types::I64, types::I64],
                None,
            ),
        ];
        for (name, ptr, _, _) in &imports {
            jit.symbol(*name, *ptr);
        }
        let mut module = JITModule::new(jit);
        let pointer = module.target_config().pointer_type();
        let mut ids = vec![];
        for (name, _, args, result) in &imports {
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(pointer));
            sig.params.extend(args.iter().copied().map(AbiParam::new));
            if let Some(ty) = result {
                sig.returns.push(AbiParam::new(*ty));
            }
            ids.push((*name, module.declare_function(name, Linkage::Import, &sig)?));
        }
        let mut context = module.make_context();
        context.func.signature.params.push(AbiParam::new(pointer));
        let entry =
            module.declare_function("scratch_main", Linkage::Local, &context.func.signature)?;
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut context.func, &mut fbc);
            let entry_block = b.create_block();
            let dispatch = b.create_block();
            let exit = b.create_block();
            let blocks: Vec<_> = program.nodes.iter().map(|_| b.create_block()).collect();
            b.append_block_params_for_function_params(entry_block);
            b.switch_to_block(entry_block);
            let vm = b.block_params(entry_block)[0];
            let funcs = ids
                .into_iter()
                .map(|(name, id)| (name, module.declare_func_in_func(id, b.func)))
                .collect();
            let vars = b.ins().load(
                pointer,
                ir::MemFlags::trusted(),
                vm,
                offset_of!(JitState, vars) as i32,
            );
            let lists = b.ins().load(
                pointer,
                ir::MemFlags::trusted(),
                vm,
                offset_of!(JitState, lists) as i32,
            );
            let mut e = Emitter {
                b,
                vm,
                vars,
                lists,
                pointer,
                funcs,
            };
            e.b.ins().jump(dispatch, &[]);
            e.b.switch_to_block(dispatch);
            let pc = e.load(types::I32, offset_of!(JitState, pc));
            let mut switch = Switch::new();
            for (id, block) in blocks.iter().enumerate() {
                switch.set_entry(id as u128, *block);
            }
            switch.emit(&mut e.b, pc, exit);
            for (id, node) in program.nodes.iter().enumerate() {
                e.b.switch_to_block(blocks[id]);
                e.checkpoint(id, exit);
                let next = blocks[node.next];
                match &node.op {
                    Op::Set(slot, x) => {
                        let x = e.expr(x);
                        e.b.ins()
                            .store(ir::MemFlags::trusted(), x, e.vars, (*slot * 8) as i32);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::Change(slot, x) => {
                        let x = e.expr(x);
                        let x = e.num(x);
                        let old = e.b.ins().load(
                            types::I64,
                            ir::MemFlags::trusted(),
                            e.vars,
                            (*slot * 8) as i32,
                        );
                        let old = e.num(old);
                        let sum = e.b.ins().fadd(old, x);
                        let bits = e.bits(sum);
                        e.b.ins()
                            .store(ir::MemFlags::trusted(), bits, e.vars, (*slot * 8) as i32);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::Action(op, a, b) => {
                        let a = e.expr(a);
                        let b = e.expr(b);
                        let op = e.i32(*op as usize);
                        e.call("action", &[op, a, b]);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::List(op, slot, a, b) => {
                        let a = e.expr(a);
                        let b = e.expr(b);
                        e.list(*op, *slot, a, b);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::Branch(c, yes, no) => {
                        let c = e.expr(c);
                        let c = e.truthy(c);
                        e.b.ins().brif(c, blocks[*yes], &[], blocks[*no], &[]);
                    }
                    Op::Jump(to, boundary) => {
                        if *boundary {
                            e.boundary(*to, blocks[*to], exit);
                        } else {
                            e.b.ins().jump(blocks[*to], &[]);
                        }
                    }
                    Op::RepeatInit(slot, count, to) => {
                        let count = e.expr(count);
                        let count = e.num(count);
                        let slot = e.i32(*slot);
                        e.call("loop_set", &[slot, count]);
                        e.b.ins().jump(blocks[*to], &[]);
                    }
                    Op::RepeatCheck(slot, yes, no) => {
                        let slot = e.i32(*slot);
                        let count = e.call("loop_get", &[slot]).unwrap();
                        let zero = e.b.ins().f64const(0.0);
                        let positive =
                            e.b.ins()
                                .fcmp(ir::condcodes::FloatCC::GreaterThan, count, zero);
                        e.b.ins()
                            .brif(positive, blocks[*yes], &[], blocks[*no], &[]);
                    }
                    Op::RepeatDec(slot, to) => {
                        let slot = e.i32(*slot);
                        e.call("loop_dec", &[slot]);
                        e.boundary(*to, blocks[*to], exit);
                    }
                    Op::ForEachCheck(slot, variable, limit, yes, no) => {
                        let limit = e.expr(limit);
                        let limit = e.num(limit);
                        let slot = e.i32(*slot);
                        let index = e.call("loop_get", &[slot]).unwrap();
                        let more =
                            e.b.ins()
                                .fcmp(ir::condcodes::FloatCC::LessThan, index, limit);
                        let advance = e.b.create_block();
                        e.b.ins().brif(more, advance, &[], blocks[*no], &[]);
                        e.b.switch_to_block(advance);
                        let one = e.b.ins().f64const(1.0);
                        let index = e.b.ins().fadd(index, one);
                        e.call("loop_set", &[slot, index]);
                        let value = e.bits(index);
                        e.b.ins().store(
                            ir::MemFlags::trusted(),
                            value,
                            e.vars,
                            (*variable * 8) as i32,
                        );
                        e.b.ins().jump(blocks[*yes], &[]);
                    }
                    Op::Yield(to) => {
                        let zero = e.b.ins().f64const(0.0);
                        let pc = e.i32(*to);
                        e.call("wait", &[zero, pc]);
                        e.b.ins().jump(exit, &[]);
                    }
                    Op::Wait(seconds) => {
                        let seconds = e.expr(seconds);
                        let seconds = e.num(seconds);
                        let pc = e.i32(node.next);
                        e.call("wait", &[seconds, pc]);
                        e.b.ins().jump(exit, &[]);
                    }
                    Op::Broadcast(name, wait) => {
                        let name = e.expr(name);
                        let wait_value = e.i32(usize::from(*wait));
                        let pc = e.i32(node.next);
                        e.call("broadcast", &[name, wait_value, pc]);
                        e.b.ins().jump(if *wait { exit } else { next }, &[]);
                    }
                    Op::Call(proc, args) => {
                        let values: Vec<_> = args.iter().map(|a| e.expr(a)).collect();
                        if !values.is_empty() {
                            let count = e.i32(values.len());
                            let data = e.call("call_args", &[count]).unwrap();
                            for (i, value) in values.into_iter().enumerate() {
                                e.b.ins().store(
                                    ir::MemFlags::trusted(),
                                    value,
                                    data,
                                    (i * 8) as i32,
                                );
                            }
                        }
                        let proc = e.i32(*proc);
                        let pc = e.i32(node.next);
                        let stop = e.call("call", &[proc, pc]).unwrap();
                        e.b.ins().brif(stop, exit, &[], dispatch, &[]);
                    }
                    Op::Return | Op::Stop(1) => {
                        let stop = e.call("return", &[]).unwrap();
                        e.b.ins().brif(stop, exit, &[], dispatch, &[]);
                    }
                    Op::Stop(mode) => {
                        let mode = e.i32(*mode as usize);
                        e.call("stop", &[mode]);
                        let pc = e.i32(node.next);
                        e.store(pc, offset_of!(JitState, pc));
                        e.b.ins().jump(exit, &[]);
                    }
                }
            }
            e.b.switch_to_block(exit);
            e.b.ins().return_(&[]);
            e.b.seal_all_blocks();
            e.b.finalize();
        }
        module
            .define_function(entry, &mut context)
            .context("compile native Scratch control flow")?;
        module.clear_context(&mut context);
        module.finalize_definitions()?;
        Ok(Self {
            module: Some(module),
            entry,
        })
    }
    pub fn execute(&self, vm: &mut Vm) {
        // SAFETY: the function was emitted above with precisely this pointer-only
        // native C signature, and self owns its executable memory for this call.
        let function: unsafe extern "C" fn(*mut Vm) = unsafe {
            std::mem::transmute(
                self.module
                    .as_ref()
                    .unwrap()
                    .get_finalized_function(self.entry),
            )
        };
        unsafe { function(vm as *mut Vm) };
    }
}
impl Drop for NativeProgram {
    fn drop(&mut self) {
        let module = self.module.take().unwrap();
        // SAFETY: execute borrows self, so no generated call can be live here.
        unsafe { module.free_memory() };
    }
}
struct Emitter<'a> {
    b: FunctionBuilder<'a>,
    vm: ir::Value,
    vars: ir::Value,
    lists: ir::Value,
    pointer: ir::Type,
    funcs: HashMap<&'static str, ir::FuncRef>,
}
impl Emitter<'_> {
    fn load(&mut self, ty: ir::Type, offset: usize) -> ir::Value {
        self.b
            .ins()
            .load(ty, ir::MemFlags::trusted(), self.vm, offset as i32)
    }
    fn store(&mut self, value: ir::Value, offset: usize) {
        self.b
            .ins()
            .store(ir::MemFlags::trusted(), value, self.vm, offset as i32);
    }
    fn checkpoint(&mut self, pc: usize, exit: ir::Block) {
        let budget = self.load(types::I32, offset_of!(JitState, budget));
        let empty = self
            .b
            .ins()
            .icmp_imm(ir::condcodes::IntCC::Equal, budget, 0);
        let slow = self.b.create_block();
        let ready = self.b.create_block();
        self.b.ins().brif(empty, slow, &[], ready, &[]);
        self.b.switch_to_block(slow);
        let pc = self.i32(pc);
        let stop = self.call("checkpoint", &[pc]).unwrap();
        self.b.ins().brif(stop, exit, &[], ready, &[]);
        self.b.switch_to_block(ready);
        let budget = self.load(types::I32, offset_of!(JitState, budget));
        let budget = self.b.ins().iadd_imm(budget, -1);
        self.store(budget, offset_of!(JitState, budget));
        let steps = self.load(types::I64, offset_of!(JitState, steps));
        let steps = self.b.ins().iadd_imm(steps, 1);
        self.store(steps, offset_of!(JitState, steps));
    }
    fn boxed(&mut self, value: ir::Value) -> ir::Value {
        let tag = self
            .b
            .ins()
            .band_imm(value, 0xfffe_0000_0000_0000u64 as i64);
        self.b.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            tag,
            crate::value::STRING_TAG as i64,
        )
    }
    fn raw_num(&mut self, bits: ir::Value) -> ir::Value {
        let n = self.b.ins().bitcast(types::F64, ir::MemFlags::new(), bits);
        let nan = self.b.ins().fcmp(ir::condcodes::FloatCC::Unordered, n, n);
        let zero = self.b.ins().f64const(0.0);
        self.b.ins().select(nan, zero, n)
    }
    fn boolean(&mut self, value: ir::Value) -> ir::Value {
        let value = self.b.ins().uextend(types::I64, value);
        self.b.ins().bor_imm(value, crate::value::BOOL_TAG as i64)
    }
    fn truthy(&mut self, value: ir::Value) -> ir::Value {
        let tag = self
            .b
            .ins()
            .band_imm(value, 0xffff_0000_0000_0000u64 as i64);
        let string = self.b.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            tag,
            crate::value::STRING_TAG as i64,
        );
        let slow = self.b.create_block();
        let other = self.b.create_block();
        let bool_block = self.b.create_block();
        let number = self.b.create_block();
        let join = self.b.create_block();
        self.b.append_block_param(join, types::I32);
        self.b.ins().brif(string, slow, &[], other, &[]);
        self.b.switch_to_block(slow);
        let result = self.call("bool", &[value]).unwrap();
        self.b.ins().jump(join, &[result.into()]);
        self.b.switch_to_block(other);
        let boolean = self.b.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            tag,
            crate::value::BOOL_TAG as i64,
        );
        self.b.ins().brif(boolean, bool_block, &[], number, &[]);
        self.b.switch_to_block(bool_block);
        let result = self.b.ins().band_imm(value, 1);
        let result = self.b.ins().ireduce(types::I32, result);
        self.b.ins().jump(join, &[result.into()]);
        self.b.switch_to_block(number);
        let n = self.raw_num(value);
        let zero = self.b.ins().f64const(0.0);
        let result = self.b.ins().fcmp(ir::condcodes::FloatCC::NotEqual, n, zero);
        let result = self.b.ins().uextend(types::I32, result);
        self.b.ins().jump(join, &[result.into()]);
        self.b.switch_to_block(join);
        self.b.block_params(join)[0]
    }
    fn list(&mut self, op: u32, slot: usize, index: ir::Value, item: ir::Value) -> ir::Value {
        use crate::lists::ListView;
        let base = slot * std::mem::size_of::<ListView>();
        if op == 6 {
            let len = self.b.ins().load(
                self.pointer,
                ir::MemFlags::trusted(),
                self.lists,
                (base + offset_of!(ListView, len)) as i32,
            );
            let len = self.b.ins().fcvt_from_uint(types::F64, len);
            return self.bits(len);
        }
        if op != 4 && op != 5 {
            let op = self.i32(op as usize);
            let slot = self.i32(slot);
            return self.call("list", &[op, slot, index, item]).unwrap();
        }
        let slow = self.b.create_block();
        let numeric = self.b.create_block();
        let valid = self.b.create_block();
        let join = self.b.create_block();
        self.b.append_block_param(join, types::I64);
        let boxed = self.boxed(index);
        self.b.ins().brif(boxed, slow, &[], numeric, &[]);
        self.b.switch_to_block(numeric);
        let n = self.b.ins().bitcast(types::F64, ir::MemFlags::new(), index);
        let n = self.b.ins().floor(n);
        let len = self.b.ins().load(
            self.pointer,
            ir::MemFlags::trusted(),
            self.lists,
            (base + offset_of!(ListView, len)) as i32,
        );
        let len = self.b.ins().fcvt_from_uint(types::F64, len);
        let one = self.b.ins().f64const(1.0);
        let lower = self
            .b
            .ins()
            .fcmp(ir::condcodes::FloatCC::GreaterThanOrEqual, n, one);
        let upper = self
            .b
            .ins()
            .fcmp(ir::condcodes::FloatCC::LessThanOrEqual, n, len);
        let in_bounds = self.b.ins().band(lower, upper);
        self.b.ins().brif(in_bounds, valid, &[], slow, &[]);
        self.b.switch_to_block(valid);
        let at = self.b.ins().fcvt_to_uint_sat(self.pointer, n);
        let at = self.b.ins().iadd_imm(at, -1);
        let at = self.b.ins().ishl_imm(at, 3);
        let data = self.b.ins().load(
            self.pointer,
            ir::MemFlags::trusted(),
            self.lists,
            (base + offset_of!(ListView, data)) as i32,
        );
        let address = self.b.ins().iadd(data, at);
        let old = self
            .b
            .ins()
            .load(types::I64, ir::MemFlags::trusted(), address, 0);
        if op == 5 {
            self.b.ins().jump(join, &[old.into()]);
        } else {
            let old_tag = self.b.ins().band_imm(old, 0xffff_0000_0000_0000u64 as i64);
            let new_tag = self.b.ins().band_imm(item, 0xffff_0000_0000_0000u64 as i64);
            let old_string = self.b.ins().icmp_imm(
                ir::condcodes::IntCC::Equal,
                old_tag,
                crate::value::STRING_TAG as i64,
            );
            let new_string = self.b.ins().icmp_imm(
                ir::condcodes::IntCC::Equal,
                new_tag,
                crate::value::STRING_TAG as i64,
            );
            let needs_roots = self.b.ins().bor(old_string, new_string);
            let write = self.b.create_block();
            self.b.ins().brif(needs_roots, slow, &[], write, &[]);
            self.b.switch_to_block(write);
            self.b
                .ins()
                .store(ir::MemFlags::trusted(), item, address, 0);
            let zero = self.b.ins().iconst(types::I64, 0);
            self.b.ins().jump(join, &[zero.into()]);
        }
        self.b.switch_to_block(slow);
        let op_value = self.i32(op as usize);
        let slot_value = self.i32(slot);
        let result = self
            .call("list", &[op_value, slot_value, index, item])
            .unwrap();
        self.b.ins().jump(join, &[result.into()]);
        self.b.switch_to_block(join);
        self.b.block_params(join)[0]
    }
    fn i32(&mut self, n: usize) -> ir::Value {
        self.b.ins().iconst(types::I32, n as i64)
    }
    fn call(&mut self, name: &str, args: &[ir::Value]) -> Option<ir::Value> {
        let mut params = vec![self.vm];
        params.extend_from_slice(args);
        let call = self.b.ins().call(self.funcs[name], &params);
        self.b.inst_results(call).first().copied()
    }
    fn num(&mut self, bits: ir::Value) -> ir::Value {
        // Numbers take a native fast path; only boxed strings/booleans cross
        // the host ABI for Scratch coercion.
        let tag = self.b.ins().band_imm(bits, 0xfffe_0000_0000_0000u64 as i64);
        let boxed = self.b.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            tag,
            crate::value::STRING_TAG as i64,
        );
        let slow = self.b.create_block();
        let fast = self.b.create_block();
        let join = self.b.create_block();
        self.b.append_block_param(join, types::F64);
        self.b.ins().brif(boxed, slow, &[], fast, &[]);
        self.b.switch_to_block(fast);
        let n = self.b.ins().bitcast(types::F64, ir::MemFlags::new(), bits);
        let nan = self.b.ins().fcmp(ir::condcodes::FloatCC::Unordered, n, n);
        let zero = self.b.ins().f64const(0.0);
        let n = self.b.ins().select(nan, zero, n);
        self.b.ins().jump(join, &[n.into()]);
        self.b.switch_to_block(slow);
        let n = self.call("number", &[bits]).unwrap();
        self.b.ins().jump(join, &[n.into()]);
        self.b.switch_to_block(join);
        self.b.block_params(join)[0]
    }
    fn bits(&mut self, n: ir::Value) -> ir::Value {
        self.b.ins().bitcast(types::I64, ir::MemFlags::new(), n)
    }
    fn boundary(&mut self, pc: usize, to: ir::Block, exit: ir::Block) {
        let pc = self.i32(pc);
        self.store(pc, offset_of!(JitState, pc));
        let stop = self.load(types::I32, offset_of!(JitState, yield_on_loop));
        self.b.ins().brif(stop, exit, &[], to, &[]);
    }
    fn expr(&mut self, expr: &Expr) -> ir::Value {
        match expr {
            Expr::Constant(bits) => self.b.ins().iconst(types::I64, *bits as i64),
            Expr::Variable(slot) => self.b.ins().load(
                types::I64,
                ir::MemFlags::trusted(),
                self.vars,
                (*slot * 8) as i32,
            ),
            Expr::Argument(slot) => {
                let args = self.load(self.pointer, offset_of!(JitState, args));
                self.b.ins().load(
                    types::I64,
                    ir::MemFlags::trusted(),
                    args,
                    (*slot * 8) as i32,
                )
            }
            Expr::State(field) => {
                let field = self.i32(*field as usize);
                self.call("state", &[field]).unwrap()
            }
            Expr::List(op, slot, arg) => {
                let arg = self.expr(arg);
                let zero = self.b.ins().iconst(types::I64, 0);
                self.list(*op, *slot, arg, zero)
            }
            Expr::Unary(op, a) => {
                let a = self.expr(a);
                if *op == 0 {
                    let a = self.truthy(a);
                    let result = self.b.ins().icmp_imm(ir::condcodes::IntCC::Equal, a, 0);
                    return self.boolean(result);
                }
                if (2..=6).contains(op) {
                    let n = self.num(a);
                    let result = match op {
                        2 => {
                            let half = self.b.ins().f64const(0.5);
                            let n = self.b.ins().fadd(n, half);
                            self.b.ins().floor(n)
                        }
                        3 => self.b.ins().fabs(n),
                        4 => self.b.ins().floor(n),
                        5 => self.b.ins().ceil(n),
                        _ => self.b.ins().sqrt(n),
                    };
                    return self.bits(result);
                }
                let op = self.i32(*op as usize);
                self.call("unary", &[op, a]).unwrap()
            }
            Expr::Binary(op, a, b) => {
                let a = self.expr(a);
                let b = self.expr(b);
                if *op == 8 || *op == 9 {
                    let a = self.truthy(a);
                    let b = self.truthy(b);
                    let result = if *op == 8 {
                        self.b.ins().band(a, b)
                    } else {
                        self.b.ins().bor(a, b)
                    };
                    return self.boolean(result);
                }
                if (5..=7).contains(op) {
                    let a_boxed = self.boxed(a);
                    let b_boxed = self.boxed(b);
                    let boxed = self.b.ins().bor(a_boxed, b_boxed);
                    let slow = self.b.create_block();
                    let fast = self.b.create_block();
                    let join = self.b.create_block();
                    self.b.append_block_param(join, types::I64);
                    self.b.ins().brif(boxed, slow, &[], fast, &[]);
                    self.b.switch_to_block(fast);
                    let na = self.raw_num(a);
                    let nb = self.raw_num(b);
                    let condition = match op {
                        5 => ir::condcodes::FloatCC::LessThan,
                        6 => ir::condcodes::FloatCC::Equal,
                        _ => ir::condcodes::FloatCC::GreaterThan,
                    };
                    let result = self.b.ins().fcmp(condition, na, nb);
                    let result = self.boolean(result);
                    self.b.ins().jump(join, &[result.into()]);
                    self.b.switch_to_block(slow);
                    let op = self.i32(*op as usize);
                    let result = self.call("binary", &[op, a, b]).unwrap();
                    self.b.ins().jump(join, &[result.into()]);
                    self.b.switch_to_block(join);
                    return self.b.block_params(join)[0];
                }
                if *op < 4 {
                    let a = self.num(a);
                    let b = self.num(b);
                    let result = match op {
                        0 => self.b.ins().fadd(a, b),
                        1 => self.b.ins().fsub(a, b),
                        2 => self.b.ins().fmul(a, b),
                        _ => self.b.ins().fdiv(a, b),
                    };
                    self.bits(result)
                } else {
                    let op = self.i32(*op as usize);
                    self.call("binary", &[op, a, b]).unwrap()
                }
            }
        }
    }
}
