use std::fmt;
use std::ops;
use std::rc::Rc;
use std::slice;

use cranelift_entity::{entity_impl, PrimaryMap};

use crate::cdsl::camel_case;
use crate::cdsl::formats::{
    FormatField, FormatRegistry, InstructionFormat, InstructionFormatIndex,
};
use crate::cdsl::operands::Operand;
use crate::cdsl::type_inference::Constraint;
use crate::cdsl::types::{LaneType, ValueType};
use crate::cdsl::typevar::TypeVar;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpcodeNumber(u32);
entity_impl!(OpcodeNumber);

pub type AllInstructions = PrimaryMap<OpcodeNumber, Instruction>;

pub struct InstructionGroupBuilder<'format_reg, 'all_inst> {
    _name: &'static str,
    _doc: &'static str,
    format_registry: &'format_reg FormatRegistry,
    all_instructions: &'all_inst mut AllInstructions,
    own_instructions: Vec<Instruction>,
}

impl<'format_reg, 'all_inst> InstructionGroupBuilder<'format_reg, 'all_inst> {
    pub fn new(
        name: &'static str,
        doc: &'static str,
        all_instructions: &'all_inst mut AllInstructions,
        format_registry: &'format_reg FormatRegistry,
    ) -> Self {
        Self {
            _name: name,
            _doc: doc,
            format_registry,
            all_instructions,
            own_instructions: Vec::new(),
        }
    }

    pub fn push(&mut self, builder: InstructionBuilder) {
        let opcode_number = OpcodeNumber(self.all_instructions.next_key().as_u32());
        let inst = builder.build(self.format_registry, opcode_number);
        // Note this clone is cheap, since Instruction is a Rc<> wrapper for InstructionContent.
        self.own_instructions.push(inst.clone());
        self.all_instructions.push(inst);
    }

    pub fn build(self) -> InstructionGroup {
        InstructionGroup {
            _name: self._name,
            _doc: self._doc,
            instructions: self.own_instructions,
        }
    }
}

/// Every instruction must belong to exactly one instruction group. A given
/// target architecture can support instructions from multiple groups, and it
/// does not necessarily support all instructions in a group.
pub struct InstructionGroup {
    _name: &'static str,
    _doc: &'static str,
    instructions: Vec<Instruction>,
}

impl InstructionGroup {
    pub fn iter(&self) -> slice::Iter<Instruction> {
        self.instructions.iter()
    }

    pub fn by_name(&self, name: &'static str) -> &Instruction {
        self.instructions
            .iter()
            .find(|inst| inst.name == name)
            .expect(&format!("unexisting instruction with name {}", name))
    }
}

pub struct PolymorphicInfo {
    pub use_typevar_operand: bool,
    pub ctrl_typevar: TypeVar,
    pub other_typevars: Vec<TypeVar>,
}

pub struct InstructionContent {
    /// Instruction mnemonic, also becomes opcode name.
    pub name: String,
    pub camel_name: String,
    pub opcode_number: OpcodeNumber,

    /// Documentation string.
    doc: String,

    /// Input operands. This can be a mix of SSA value operands and other operand kinds.
    pub operands_in: Vec<Operand>,
    /// Output operands. The output operands must be SSA values or `variable_args`.
    pub operands_out: Vec<Operand>,
    /// Instruction-specific TypeConstraints.
    pub constraints: Vec<Constraint>,

    /// Instruction format, automatically derived from the input operands.
    pub format: InstructionFormatIndex,

    /// One of the input or output operands is a free type variable. None if the instruction is not
    /// polymorphic, set otherwise.
    pub polymorphic_info: Option<PolymorphicInfo>,

    pub value_opnums: Vec<usize>,
    pub value_results: Vec<usize>,
    pub imm_opnums: Vec<usize>,

    /// True for instructions that terminate the EBB.
    pub is_terminator: bool,
    /// True for all branch or jump instructions.
    pub is_branch: bool,
    /// True for all indirect branch or jump instructions.',
    pub is_indirect_branch: bool,
    /// Is this a call instruction?
    pub is_call: bool,
    /// Is this a return instruction?
    pub is_return: bool,
    /// Is this a ghost instruction?
    pub is_ghost: bool,
    /// Can this instruction read from memory?
    pub can_load: bool,
    /// Can this instruction write to memory?
    pub can_store: bool,
    /// Can this instruction cause a trap?
    pub can_trap: bool,
    /// Does this instruction have other side effects besides can_* flags?
    pub other_side_effects: bool,
    /// Does this instruction write to CPU flags?
    pub writes_cpu_flags: bool,
}

#[derive(Clone)]
pub struct Instruction {
    content: Rc<InstructionContent>,
}

impl ops::Deref for Instruction {
    type Target = InstructionContent;
    fn deref(&self) -> &Self::Target {
        &*self.content
    }
}

impl Instruction {
    pub fn snake_name(&self) -> &str {
        if self.name == "return" {
            "return_"
        } else {
            &self.name
        }
    }

    pub fn doc_comment_first_line(&self) -> &str {
        for line in self.doc.split("\n") {
            let stripped = line.trim();
            if stripped.len() > 0 {
                return stripped;
            }
        }
        ""
    }

    pub fn all_typevars(&self) -> Vec<&TypeVar> {
        match &self.polymorphic_info {
            Some(poly) => {
                let mut result = vec![&poly.ctrl_typevar];
                result.extend(&poly.other_typevars);
                result
            }
            None => Vec::new(),
        }
    }

    pub fn bind(&self, lane_type: impl Into<LaneType>) -> BoundInstruction {
        bind(self.clone(), Some(lane_type.into()), Vec::new())
    }
    pub fn bind_any(&self) -> BoundInstruction {
        bind(self.clone(), None, Vec::new())
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.operands_out.len() > 0 {
            let operands_out = self
                .operands_out
                .iter()
                .map(|op| op.name)
                .collect::<Vec<_>>()
                .join(", ");
            fmt.write_str(&operands_out)?;
            fmt.write_str(" = ")?;
        }

        fmt.write_str(&self.name)?;

        if self.operands_in.len() > 0 {
            let operands_in = self
                .operands_in
                .iter()
                .map(|op| op.name)
                .collect::<Vec<_>>()
                .join(", ");
            fmt.write_str(" ")?;
            fmt.write_str(&operands_in)?;
        }

        Ok(())
    }
}

pub struct InstructionBuilder {
    name: String,
    doc: String,
    operands_in: Option<Vec<Operand>>,
    operands_out: Option<Vec<Operand>>,
    constraints: Option<Vec<Constraint>>,

    // See Instruction comments for the meaning of these fields.
    is_terminator: bool,
    is_branch: bool,
    is_indirect_branch: bool,
    is_call: bool,
    is_return: bool,
    is_ghost: bool,
    can_load: bool,
    can_store: bool,
    can_trap: bool,
    other_side_effects: bool,
}

impl InstructionBuilder {
    pub fn new<S: Into<String>>(name: S, doc: S) -> Self {
        Self {
            name: name.into(),
            doc: doc.into(),
            operands_in: None,
            operands_out: None,
            constraints: None,

            is_terminator: false,
            is_branch: false,
            is_indirect_branch: false,
            is_call: false,
            is_return: false,
            is_ghost: false,
            can_load: false,
            can_store: false,
            can_trap: false,
            other_side_effects: false,
        }
    }

    pub fn operands_in(mut self, operands: Vec<&Operand>) -> Self {
        assert!(self.operands_in.is_none());
        self.operands_in = Some(operands.iter().map(|x| (*x).clone()).collect());
        self
    }
    pub fn operands_out(mut self, operands: Vec<&Operand>) -> Self {
        assert!(self.operands_out.is_none());
        self.operands_out = Some(operands.iter().map(|x| (*x).clone()).collect());
        self
    }
    pub fn constraints(mut self, constraints: Vec<Constraint>) -> Self {
        assert!(self.constraints.is_none());
        self.constraints = Some(constraints);
        self
    }

    pub fn is_terminator(mut self, val: bool) -> Self {
        self.is_terminator = val;
        self
    }
    pub fn is_branch(mut self, val: bool) -> Self {
        self.is_branch = val;
        self
    }
    pub fn is_indirect_branch(mut self, val: bool) -> Self {
        self.is_indirect_branch = val;
        self
    }
    pub fn is_call(mut self, val: bool) -> Self {
        self.is_call = val;
        self
    }
    pub fn is_return(mut self, val: bool) -> Self {
        self.is_return = val;
        self
    }
    pub fn is_ghost(mut self, val: bool) -> Self {
        self.is_ghost = val;
        self
    }
    pub fn can_load(mut self, val: bool) -> Self {
        self.can_load = val;
        self
    }
    pub fn can_store(mut self, val: bool) -> Self {
        self.can_store = val;
        self
    }
    pub fn can_trap(mut self, val: bool) -> Self {
        self.can_trap = val;
        self
    }
    pub fn other_side_effects(mut self, val: bool) -> Self {
        self.other_side_effects = val;
        self
    }

    fn build(self, format_registry: &FormatRegistry, opcode_number: OpcodeNumber) -> Instruction {
        let operands_in = self.operands_in.unwrap_or_else(Vec::new);
        let operands_out = self.operands_out.unwrap_or_else(Vec::new);

        let format_index = format_registry.lookup(&operands_in);

        let mut value_opnums = Vec::new();
        let mut imm_opnums = Vec::new();
        for (i, op) in operands_in.iter().enumerate() {
            if op.is_value() {
                value_opnums.push(i);
            } else if op.is_immediate() {
                imm_opnums.push(i);
            } else {
                assert!(op.is_varargs());
            }
        }

        let mut value_results = Vec::new();
        for (i, op) in operands_out.iter().enumerate() {
            if op.is_value() {
                value_results.push(i);
            }
        }

        let format = format_registry.get(format_index);
        let polymorphic_info =
            verify_polymorphic(&operands_in, &operands_out, &format, &value_opnums);

        // Infer from output operands whether an instruciton clobbers CPU flags or not.
        let writes_cpu_flags = operands_out.iter().any(|op| op.is_cpu_flags());

        let camel_name = camel_case(&self.name);

        Instruction {
            content: Rc::new(InstructionContent {
                name: self.name,
                camel_name,
                opcode_number,
                doc: self.doc,
                operands_in,
                operands_out,
                constraints: self.constraints.unwrap_or_else(Vec::new),
                format: format_index,
                polymorphic_info,
                value_opnums,
                value_results,
                imm_opnums,
                is_terminator: self.is_terminator,
                is_branch: self.is_branch,
                is_indirect_branch: self.is_indirect_branch,
                is_call: self.is_call,
                is_return: self.is_return,
                is_ghost: self.is_ghost,
                can_load: self.can_load,
                can_store: self.can_store,
                can_trap: self.can_trap,
                other_side_effects: self.other_side_effects,
                writes_cpu_flags,
            }),
        }
    }
}

/// A thin wrapper like Option<ValueType>, but with more precise semantics.
#[derive(Clone)]
pub enum ValueTypeOrAny {
    ValueType(ValueType),
    Any,
}

impl ValueTypeOrAny {
    pub fn expect(self, msg: &str) -> ValueType {
        match self {
            ValueTypeOrAny::ValueType(vt) => vt,
            ValueTypeOrAny::Any => panic!(format!("Unexpected Any: {}", msg)),
        }
    }
}

#[derive(Clone)]
pub struct BoundInstruction {
    pub inst: Instruction,
    pub value_types: Vec<ValueTypeOrAny>,
}

impl BoundInstruction {
    pub fn bind(self, lane_type: impl Into<LaneType>) -> BoundInstruction {
        bind(self.inst, Some(lane_type.into()), self.value_types)
    }
    pub fn bind_any(self) -> BoundInstruction {
        bind(self.inst, None, self.value_types)
    }
}

/// Check if this instruction is polymorphic, and verify its use of type variables.
fn verify_polymorphic(
    operands_in: &Vec<Operand>,
    operands_out: &Vec<Operand>,
    format: &InstructionFormat,
    value_opnums: &Vec<usize>,
) -> Option<PolymorphicInfo> {
    // The instruction is polymorphic if it has one free input or output operand.
    let is_polymorphic = operands_in
        .iter()
        .any(|op| op.is_value() && op.type_var().unwrap().free_typevar().is_some())
        || operands_out
            .iter()
            .any(|op| op.is_value() && op.type_var().unwrap().free_typevar().is_some());

    if !is_polymorphic {
        return None;
    }

    // Verify the use of type variables.
    let mut use_typevar_operand = false;
    let mut ctrl_typevar = None;
    let mut other_typevars = None;
    let mut maybe_error_message = None;

    let tv_op = format.typevar_operand;
    if let Some(tv_op) = tv_op {
        if tv_op < value_opnums.len() {
            let op_num = value_opnums[tv_op];
            let tv = operands_in[op_num].type_var().unwrap();
            let free_typevar = tv.free_typevar();
            if (free_typevar.is_some() && tv == &free_typevar.unwrap())
                || tv.singleton_type().is_some()
            {
                match verify_ctrl_typevar(tv, &value_opnums, &operands_in, &operands_out) {
                    Ok(typevars) => {
                        other_typevars = Some(typevars);
                        ctrl_typevar = Some(tv.clone());
                        use_typevar_operand = true;
                    }
                    Err(error_message) => {
                        maybe_error_message = Some(error_message);
                    }
                }
            }
        }
    };

    if !use_typevar_operand {
        if operands_out.len() == 0 {
            match maybe_error_message {
                Some(msg) => panic!(msg),
                None => panic!("typevar_operand must be a free type variable"),
            }
        }

        let tv = operands_out[0].type_var().unwrap();
        let free_typevar = tv.free_typevar();
        if free_typevar.is_some() && tv != &free_typevar.unwrap() {
            panic!("first result must be a free type variable");
        }

        other_typevars =
            Some(verify_ctrl_typevar(tv, &value_opnums, &operands_in, &operands_out).unwrap());
        ctrl_typevar = Some(tv.clone());
    }

    // rustc is not capable to determine this statically, so enforce it with options.
    assert!(ctrl_typevar.is_some());
    assert!(other_typevars.is_some());

    Some(PolymorphicInfo {
        use_typevar_operand,
        ctrl_typevar: ctrl_typevar.unwrap(),
        other_typevars: other_typevars.unwrap(),
    })
}

/// Verify that the use of TypeVars is consistent with `ctrl_typevar` as the controlling type
/// variable.
///
/// All polymorhic inputs must either be derived from `ctrl_typevar` or be independent free type
/// variables only used once.
///
/// All polymorphic results must be derived from `ctrl_typevar`.
///
/// Return a vector of other type variables used, or panics.
fn verify_ctrl_typevar(
    ctrl_typevar: &TypeVar,
    value_opnums: &Vec<usize>,
    operands_in: &Vec<Operand>,
    operands_out: &Vec<Operand>,
) -> Result<Vec<TypeVar>, String> {
    let mut other_typevars = Vec::new();

    // Check value inputs.
    for &op_num in value_opnums {
        let typ = operands_in[op_num].type_var();

        let tv = if let Some(typ) = typ {
            typ.free_typevar()
        } else {
            None
        };

        // Non-polymorphic or derived from ctrl_typevar is OK.
        let tv = match tv {
            Some(tv) => {
                if &tv == ctrl_typevar {
                    continue;
                }
                tv
            }
            None => continue,
        };

        // No other derived typevars allowed.
        if typ.is_some() && typ.unwrap() != &tv {
            return Err(format!(
                "{:?}: type variable {} must be derived from {:?}",
                operands_in[op_num],
                typ.unwrap().name,
                ctrl_typevar
            ));
        }

        // Other free type variables can only be used once each.
        for other_tv in &other_typevars {
            if &tv == other_tv {
                return Err(format!(
                    "type variable {} can't be used more than once",
                    tv.name
                ));
            }
        }

        other_typevars.push(tv);
    }

    // Check outputs.
    for result in operands_out {
        if !result.is_value() {
            continue;
        }

        let typ = result.type_var().unwrap();
        let tv = typ.free_typevar();

        // Non-polymorphic or derived form ctrl_typevar is OK.
        if tv.is_none() || &tv.unwrap() == ctrl_typevar {
            continue;
        }

        return Err("type variable in output not derived from ctrl_typevar".into());
    }

    Ok(other_typevars)
}

pub enum FormatPredicateKind {
    /// Is the field member equal to the expected value (stored here)?
    IsEqual(String),
}

pub struct FormatPredicateNode {
    _format_name: &'static str,
    field_name: &'static str,
    kind: FormatPredicateKind,
}

impl FormatPredicateNode {
    fn new(
        _format_name: &'static str,
        field_name: &'static str,
        kind: FormatPredicateKind,
    ) -> Self {
        Self {
            _format_name,
            field_name,
            kind,
        }
    }
    fn rust_predicate(&self) -> String {
        match &self.kind {
            FormatPredicateKind::IsEqual(arg) => {
                format!("crate::predicates::is_equal({}, {})", self.field_name, arg)
            }
        }
    }
}

pub enum TypePredicateNode {
    /// Is the value argument (at the index designated by the first member) the same type as the
    /// type name (second member)?
    TypeVarCheck(usize, String),

    /// Is the controlling type variable the same type as the one designated by the type name
    /// (only member)?
    CtrlTypeVarCheck(String),
}

impl TypePredicateNode {
    fn rust_predicate(&self) -> String {
        match self {
            TypePredicateNode::TypeVarCheck(index, value_type_name) => format!(
                "func.dfg.value_type(args[{}]) == {}",
                index, value_type_name
            ),
            TypePredicateNode::CtrlTypeVarCheck(value_type_name) => {
                format!("func.dfg.ctrl_typevar(inst) == {}", value_type_name)
            }
        }
    }
}

/// A basic node in an instruction predicate: either an atom, or an AND of two conditions.
pub enum InstructionPredicateNode {
    FormatPredicate(FormatPredicateNode),

    TypePredicate(TypePredicateNode),

    /// A combination of two other predicates.
    And(Vec<InstructionPredicateNode>),
}

impl InstructionPredicateNode {
    fn rust_predicate(&self) -> String {
        match self {
            InstructionPredicateNode::FormatPredicate(node) => node.rust_predicate(),
            InstructionPredicateNode::TypePredicate(node) => node.rust_predicate(),
            InstructionPredicateNode::And(nodes) => nodes
                .iter()
                .map(|x| x.rust_predicate())
                .collect::<Vec<_>>()
                .join(" &&\n"),
        }
    }
}

pub struct InstructionPredicate {
    node: Option<InstructionPredicateNode>,
}

impl InstructionPredicate {
    pub fn new() -> Self {
        Self { node: None }
    }

    pub fn new_typevar_check(
        inst: &Instruction,
        type_var: &TypeVar,
        value_type: &ValueType,
    ) -> InstructionPredicateNode {
        let index = inst
            .value_opnums
            .iter()
            .enumerate()
            .filter(|(_, &op_num)| inst.operands_in[op_num].type_var().unwrap() == type_var)
            .next()
            .unwrap()
            .0;
        InstructionPredicateNode::TypePredicate(TypePredicateNode::TypeVarCheck(
            index,
            value_type.rust_name(),
        ))
    }

    pub fn new_is_field_equal(
        format_name: &'static str,
        format_field: &FormatField,
        imm_value: String,
    ) -> InstructionPredicateNode {
        InstructionPredicateNode::FormatPredicate(FormatPredicateNode::new(
            format_name,
            format_field.member,
            FormatPredicateKind::IsEqual(imm_value),
        ))
    }

    pub fn new_ctrl_typevar_check(value_type: &ValueType) -> InstructionPredicateNode {
        InstructionPredicateNode::TypePredicate(TypePredicateNode::CtrlTypeVarCheck(
            value_type.rust_name(),
        ))
    }

    pub fn and(mut self, new_node: InstructionPredicateNode) -> Self {
        let node = self.node;
        let mut and_nodes = match node {
            Some(node) => match node {
                InstructionPredicateNode::And(nodes) => nodes,
                _ => vec![node],
            },
            _ => Vec::new(),
        };
        and_nodes.push(new_node);
        self.node = Some(InstructionPredicateNode::And(and_nodes));
        self
    }

    pub fn rust_predicate(&self) -> String {
        match &self.node {
            Some(root) => root.rust_predicate(),
            None => "true".into(),
        }
    }
}

/// An instruction specification, containing an instruction that has bound types or not.
pub enum InstSpec {
    Inst(Instruction),
    Bound(BoundInstruction),
}

impl InstSpec {
    pub fn inst(&self) -> &Instruction {
        match &self {
            InstSpec::Inst(inst) => inst,
            InstSpec::Bound(bound_inst) => &bound_inst.inst,
        }
    }
    pub fn bind(&self, lane_type: impl Into<LaneType>) -> BoundInstruction {
        match self {
            InstSpec::Inst(inst) => inst.bind(lane_type),
            InstSpec::Bound(inst) => inst.clone().bind(lane_type),
        }
    }
}

impl Into<InstSpec> for &Instruction {
    fn into(self) -> InstSpec {
        InstSpec::Inst(self.clone())
    }
}

impl Into<InstSpec> for BoundInstruction {
    fn into(self) -> InstSpec {
        InstSpec::Bound(self)
    }
}

/// Helper bind reused by {Bound,}Instruction::bind.
fn bind(
    inst: Instruction,
    lane_type: Option<LaneType>,
    mut value_types: Vec<ValueTypeOrAny>,
) -> BoundInstruction {
    match lane_type {
        Some(lane_type) => {
            value_types.push(ValueTypeOrAny::ValueType(lane_type.into()));
        }
        None => {
            value_types.push(ValueTypeOrAny::Any);
        }
    }

    match &inst.polymorphic_info {
        Some(poly) => {
            assert!(
                value_types.len() <= 1 + poly.other_typevars.len(),
                format!("trying to bind too many types for {}", inst.name)
            );
        }
        None => {
            panic!(format!(
                "trying to bind a type for {} which is not a polymorphic instruction",
                inst.name
            ));
        }
    }

    BoundInstruction { inst, value_types }
}