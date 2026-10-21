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
            ("pc", rt_pc as *const u8, vec![], Some(types::I32)),
            (
                "checkpoint",
                rt_checkpoint as *const u8,
                vec![types::I32],
                Some(types::I32),
            ),
            (
                "boundary",
                rt_boundary as *const u8,
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
                "get",
                rt_get as *const u8,
                vec![types::I32],
                Some(types::I64),
            ),
            (
                "set",
                rt_set as *const u8,
                vec![types::I32, types::I64],
                None,
            ),
            (
                "arg",
                rt_arg as *const u8,
                vec![types::I32],
                Some(types::I64),
            ),
            ("push_arg", rt_push_arg as *const u8, vec![types::I64], None),
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
            let mut e = Emitter { b, vm, funcs };
            e.b.ins().jump(dispatch, &[]);
            e.b.switch_to_block(dispatch);
            let pc = e.call("pc", &[]).unwrap();
            let mut switch = Switch::new();
            for (id, block) in blocks.iter().enumerate() {
                switch.set_entry(id as u128, *block);
            }
            switch.emit(&mut e.b, pc, exit);
            for (id, node) in program.nodes.iter().enumerate() {
                e.b.switch_to_block(blocks[id]);
                let pc = e.i32(id);
                let stop = e.call("checkpoint", &[pc]).unwrap();
                let body = e.b.create_block();
                e.b.ins().brif(stop, exit, &[], body, &[]);
                e.b.switch_to_block(body);
                let next = blocks[node.next];
                match &node.op {
                    Op::Set(slot, x) => {
                        let x = e.expr(x);
                        let slot = e.i32(*slot);
                        e.call("set", &[slot, x]);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::Change(slot, x) => {
                        let x = e.expr(x);
                        let x = e.num(x);
                        let slot = e.i32(*slot);
                        let old = e.call("get", &[slot]).unwrap();
                        let old = e.num(old);
                        let sum = e.b.ins().fadd(old, x);
                        let bits = e.bits(sum);
                        e.call("set", &[slot, bits]);
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
                        let op = e.i32(*op as usize);
                        let slot = e.i32(*slot);
                        e.call("list", &[op, slot, a, b]);
                        e.b.ins().jump(next, &[]);
                    }
                    Op::Branch(c, yes, no) => {
                        let c = e.expr(c);
                        let c = e.call("bool", &[c]).unwrap();
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
                        for a in args {
                            let a = e.expr(a);
                            e.call("push_arg", &[a]);
                        }
                        let proc = e.i32(*proc);
                        let pc = e.i32(node.next);
                        let stop = e.call("call", &[proc, pc]).unwrap();
                        e.b.ins().brif(stop, exit, &[], dispatch, &[]);
                    }
                    Op::Return => {
                        let stop = e.call("return", &[]).unwrap();
                        e.b.ins().brif(stop, exit, &[], dispatch, &[]);
                    }
                    Op::Stop(mode) => {
                        let mode = e.i32(*mode as usize);
                        e.call("stop", &[mode]);
                        let pc = e.i32(node.next);
                        e.call("boundary", &[pc]);
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
    funcs: HashMap<&'static str, ir::FuncRef>,
}
impl Emitter<'_> {
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
        let stop = self.call("boundary", &[pc]).unwrap();
        self.b.ins().brif(stop, exit, &[], to, &[]);
    }
    fn expr(&mut self, expr: &Expr) -> ir::Value {
        match expr {
            Expr::Constant(bits) => self.b.ins().iconst(types::I64, *bits as i64),
            Expr::Variable(slot) => {
                let slot = self.i32(*slot);
                self.call("get", &[slot]).unwrap()
            }
            Expr::Argument(slot) => {
                let slot = self.i32(*slot);
                self.call("arg", &[slot]).unwrap()
            }
            Expr::State(field) => {
                let field = self.i32(*field as usize);
                self.call("state", &[field]).unwrap()
            }
            Expr::List(op, slot, arg) => {
                let arg = self.expr(arg);
                let op = self.i32(*op as usize);
                let slot = self.i32(*slot);
                let zero = self.b.ins().iconst(types::I64, 0);
                self.call("list", &[op, slot, arg, zero]).unwrap()
            }
            Expr::Unary(op, a) => {
                let a = self.expr(a);
                let op = self.i32(*op as usize);
                self.call("unary", &[op, a]).unwrap()
            }
            Expr::Binary(op, a, b) => {
                let a = self.expr(a);
                let b = self.expr(b);
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
