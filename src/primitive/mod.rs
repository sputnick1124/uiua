//! Primitive definitions and top-level implementations
//!
//! For the meat of the actual array algorithms, see [`crate::algorithm`].

mod defs;
pub use defs::*;

use std::{
    borrow::{BorrowMut, Cow},
    cell::RefCell,
    f64::{
        consts::{PI, TAU},
        INFINITY,
    },
    fmt::{self},
    sync::{
        atomic::{self, AtomicUsize},
        OnceLock,
    },
};

use enum_iterator::{all, Sequence};
use once_cell::sync::Lazy;
use rand::prelude::*;
use serde::*;

use crate::{
    algorithm::{self, loops, reduce, table, zip},
    array::Array,
    boxed::Boxed,
    check::instrs_signature,
    lex::AsciiToken,
    sys::*,
    value::*,
    FunctionId, Signature, Uiua, UiuaError, UiuaResult,
};

/// Categories of primitives
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Sequence)]
#[allow(missing_docs)]
pub enum PrimClass {
    Stack,
    Constant,
    MonadicPervasive,
    DyadicPervasive,
    MonadicArray,
    DyadicArray,
    IteratingModifier,
    AggregatingModifier,
    InversionModifier,
    OtherModifier,
    Planet,
    Map,
    Local,
    Misc,
    Sys(SysOpClass),
}

impl PrimClass {
    /// Get an iterator over all primitive classes
    pub fn all() -> impl Iterator<Item = Self> {
        all()
    }
    /// Check if this class is pervasive
    pub fn is_pervasive(&self) -> bool {
        matches!(
            self,
            PrimClass::MonadicPervasive | PrimClass::DyadicPervasive
        )
    }
    /// Get an iterator over all primitives in this class
    pub fn primitives(self) -> impl Iterator<Item = Primitive> {
        Primitive::all().filter(move |prim| prim.class() == self)
    }
}

/// The names of a primitive
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrimNames {
    /// The text name
    pub text: &'static str,
    /// An ASCII token that formats to the primitive
    pub ascii: Option<AsciiToken>,
    /// The primitive's glyph
    pub glyph: Option<char>,
}

impl From<&'static str> for PrimNames {
    fn from(text: &'static str) -> Self {
        Self {
            text,
            ascii: None,
            glyph: None,
        }
    }
}
impl From<(&'static str, char)> for PrimNames {
    fn from((text, glyph): (&'static str, char)) -> Self {
        Self {
            text,
            ascii: None,
            glyph: Some(glyph),
        }
    }
}
impl From<(&'static str, AsciiToken, char)> for PrimNames {
    fn from((text, ascii, glyph): (&'static str, AsciiToken, char)) -> Self {
        Self {
            text,
            ascii: Some(ascii),
            glyph: Some(glyph),
        }
    }
}

impl fmt::Display for Primitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(c) = self.glyph() {
            write!(f, "{}", c)
        } else if let Some(s) = self.ascii() {
            write!(f, "{}", s)
        } else {
            write!(f, "{}", self.name())
        }
    }
}

impl fmt::Display for ImplPrimitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ImplPrimitive::*;
        use Primitive::*;
        match self {
            InverseBits => write!(f, "{Un}{Bits}"),
            InvWhere => write!(f, "{Un}{Where}"),
            InvCouple => write!(f, "{Un}{Couple}"),
            InvMap => write!(f, "{Un}{Map}"),
            InvAtan => write!(f, "{Un}{Atan}"),
            InvComplex => write!(f, "{Un}{Complex}"),
            InvUtf => write!(f, "{Un}{Utf}"),
            InvParse => write!(f, "{Un}{Parse}"),
            InvFix => write!(f, "{Un}{Fix}"),
            InvScan => write!(f, "{Un}{Scan}"),
            InvTrace => write!(f, "{Un}{Trace}"),
            InvStack => write!(f, "{Un}{Stack}"),
            InvDump => write!(f, "{Un}{Dump}"),
            InvBox => write!(f, "{Un}{Box}"),
            Untake => write!(f, "{Un}{Take}"),
            Undrop => write!(f, "{Un}{Drop}"),
            Unselect => write!(f, "{Un}{Select}"),
            Unpick => write!(f, "{Un}{Pick}"),
            Unpartition => write!(f, "{Un}{Partition}"),
            Cos => write!(f, "{Sin}{Add}{Eta}"),
            Asin => write!(f, "{Un}{Sin}"),
            Acos => write!(f, "{Un}{Cos}"),
            Last => write!(f, "{First}{Reverse}"),
            Unfirst => write!(f, "{Un}{First}"),
            Unlast => write!(f, "{Un}{Last}"),
            Unkeep => write!(f, "{Un}{Keep}"),
            Unrerank => write!(f, "{Un}{Rerank}"),
            Unreshape => write!(f, "{Un}{Reshape}"),
            Ungroup => write!(f, "{Un}{Group}"),
            Unjoin => write!(f, "{Un}{Join}"),
            FirstMinIndex => write!(f, "{First}{Rise}"),
            FirstMaxIndex => write!(f, "{First}{Fall}"),
            LastMinIndex => write!(f, "{First}{Reverse}{Rise}"),
            LastMaxIndex => write!(f, "{First}{Reverse}{Fall}"),
            FirstWhere => write!(f, "{First}{Where}"),
            SortUp => write!(f, "{Select}{Rise}{Dup}"),
            SortDown => write!(f, "{Select}{Fall}{Dup}"),
            Primes => write!(f, "{Un}{Reduce}{Mul}"),
            ReplaceRand => write!(f, "{Gap}{Rand}"),
            ReplaceRand2 => write!(f, "{Gap}{Gap}{Rand}"),
            ReduceContent => write!(f, "{Reduce}{Content}"),
            &TransposeN(n) => {
                if n < 0 {
                    write!(f, "{Un}(")?;
                }
                for _ in 0..n.unsigned_abs() {
                    write!(f, "{Transpose}")?;
                }
                if n < 0 {
                    write!(f, ")")?;
                }
                Ok(())
            }
        }
    }
}

macro_rules! constant {
    ($name:ident, $value:expr) => {
        fn $name() -> Value {
            thread_local! {
                #[allow(non_upper_case_globals)]
                static $name: Value = $value.into();
            }
            $name.with(Value::clone)
        }
    };
}

constant!(eta, PI / 2.0);
constant!(pi, PI);
constant!(tau, TAU);
constant!(inf, INFINITY);

/// A wrapper that nicely prints a `Primitive`
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FormatPrimitive(pub Primitive);

impl fmt::Debug for FormatPrimitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for FormatPrimitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.glyph().is_none() {
            self.0.fmt(f)
        } else {
            write!(f, "{} {}", self.0, self.0.name())
        }
    }
}

impl Primitive {
    /// Get an iterator over all primitives
    pub fn all() -> impl Iterator<Item = Self> + Clone {
        all()
    }
    /// Get an iterator over all non-deprecated primitives
    pub fn non_deprecated() -> impl Iterator<Item = Self> + Clone {
        Self::all().filter(|p| !p.is_deprecated())
    }
    /// Get the primitive's name
    ///
    /// This is the name that is used for formatting
    pub fn name(&self) -> &'static str {
        self.names().text
    }
    /// Get the ASCII token that formats to the primitive
    pub fn ascii(&self) -> Option<AsciiToken> {
        self.names().ascii
    }
    /// Get the primitive's glyph
    pub fn glyph(&self) -> Option<char> {
        self.names().glyph
    }
    /// Find a primitive by its text name
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().find(|p| p.name() == name)
    }
    /// Find a primitive by its ASCII token
    pub fn from_ascii(s: AsciiToken) -> Option<Self> {
        Self::all().find(|p| p.ascii() == Some(s))
    }
    /// Find a primitive by its glyph
    pub fn from_glyph(c: char) -> Option<Self> {
        Self::all().find(|p| p.glyph() == Some(c))
    }
    /// Get the primitive's signature, if it is always well-defined
    pub fn signature(&self) -> Option<Signature> {
        let (args, outputs) = self.args().zip(self.outputs())?;
        Some(Signature { args, outputs })
    }
    /// Check if this primitive is a modifier
    pub fn is_modifier(&self) -> bool {
        self.modifier_args().is_some()
    }
    /// Check if this primitive is a constant
    pub fn is_constant(&self) -> bool {
        self.constant().is_some()
    }
    /// Get the a constant's value
    pub fn constant(&self) -> Option<f64> {
        use Primitive::*;
        match self {
            Eta => Some(PI / 2.0),
            Pi => Some(PI),
            Tau => Some(TAU),
            Infinity => Some(INFINITY),
            _ => None,
        }
    }
    /// Get a pretty-printable wrapper for this primitive
    pub fn format(&self) -> FormatPrimitive {
        FormatPrimitive(*self)
    }
    pub(crate) fn deprecation_suggestion(&self) -> Option<String> {
        match self {
            Primitive::Unpack => Some(format!("use {} instead", Primitive::Content.format())),
            Primitive::Cross => Some(format!("use {} instead", Primitive::Table.format())),
            Primitive::Cascade => Some(format!("use {} instead", Primitive::Fork.format())),
            Primitive::Rectify => Some(String::new()),
            _ => None,
        }
    }
    /// Check if this primitive is experimental
    pub fn is_experimental(&self) -> bool {
        use Primitive::*;
        matches!(
            self,
            Rectify
                | This
                | Recur
                | All
                | Cascade
                | Map
                | Insert
                | Has
                | Get
                | Remove
                | Bind
                | Sys(SysOp::FFI)
        )
    }
    /// Check if this primitive is deprecated
    pub fn is_deprecated(&self) -> bool {
        self.deprecation_suggestion().is_some()
    }
    /// Try to parse a primitive from a name prefix
    pub fn from_format_name(name: &str) -> Option<Self> {
        if name.chars().any(char::is_uppercase) {
            return None;
        }
        if name.len() < 2 {
            return None;
        }
        match name {
            "id" => return Some(Primitive::Identity),
            "ga" => return Some(Primitive::Gap),
            "pi" => return Some(Primitive::Pi),
            "ran" => return Some(Primitive::Range),
            "tra" => return Some(Primitive::Transpose),
            "par" => return Some(Primitive::Partition),
            _ => {}
        }
        if let Some(prim) = Primitive::non_deprecated().find(|p| p.name() == name) {
            return Some(prim);
        }
        if name.len() < 3 {
            return None;
        }
        let mut matching = Primitive::non_deprecated()
            .filter(|p| p.glyph().is_some_and(|u| !u.is_ascii()) && p.name().starts_with(name));
        let res = matching.next()?;
        let exact_match = res.name() == name;
        (exact_match || matching.next().is_none()).then_some(res)
    }
    /// Try to parse multiple primitives from the concatenation of their name prefixes
    pub fn from_format_name_multi(name: &str) -> Option<Vec<(Self, &str)>> {
        let mut indices: Vec<usize> = name.char_indices().map(|(i, _)| i).collect();
        if indices.len() < 2 {
            return None;
        }
        indices.push(name.len());
        // Forward parsing
        let mut prims = Vec::new();
        let mut start = 0;
        'outer: loop {
            if start == indices.len() {
                return Some(prims);
            }
            let start_index = indices[start];
            for len in (2..=indices.len() - start).rev() {
                let end_index = indices.get(start + len).copied().unwrap_or(name.len());
                if end_index - start_index < 2 {
                    continue;
                }
                let sub_name = &name[start_index..end_index];
                if let Some(p) = Primitive::from_format_name(sub_name) {
                    // Normal primitive matching
                    prims.push((p, sub_name));
                    start += len;
                    continue 'outer;
                } else if sub_name
                    .strip_prefix('f')
                    .unwrap_or(sub_name)
                    .strip_suffix(['i', 'p'])
                    .unwrap_or(sub_name)
                    .chars()
                    .all(|c| "gd".contains(c))
                {
                    // 1-letter planet notation
                    for (i, c) in sub_name.char_indices() {
                        let prim = match c {
                            'f' => Primitive::Fork,
                            'g' => Primitive::Gap,
                            'd' => Primitive::Dip,
                            'i' => Primitive::Identity,
                            'p' => Primitive::Pop,
                            _ => unreachable!(),
                        };
                        prims.push((prim, &sub_name[i..i + 1]))
                    }
                    start += len;
                    continue 'outer;
                }
            }
            break;
        }
        // Backward parsing
        prims.clear();
        let mut end = indices.len() - 1;
        'outer: loop {
            if end == 0 {
                prims.reverse();
                return Some(prims);
            }
            let end_index = indices[end];
            for len in (2..=end).rev() {
                let start_index = indices.get(end - len).copied().unwrap_or(0);
                let sub_name = &name[start_index..end_index];
                if let Some(p) = Primitive::from_format_name(sub_name) {
                    prims.push((p, sub_name));
                    end -= len;
                    continue 'outer;
                }
            }
            break;
        }
        None
    }
    /// Execute the primitive
    pub fn run(&self, env: &mut Uiua) -> UiuaResult {
        match self {
            Primitive::Eta => env.push(eta()),
            Primitive::Pi => env.push(pi()),
            Primitive::Tau => env.push(tau()),
            Primitive::Infinity => env.push(inf()),
            Primitive::Identity => env.touch_array_stack(1),
            Primitive::Not => env.monadic_env(Value::not)?,
            Primitive::Neg => env.monadic_env(Value::neg)?,
            Primitive::Abs => env.monadic_env(Value::abs)?,
            Primitive::Sign => env.monadic_env(Value::sign)?,
            Primitive::Sqrt => env.monadic_env(Value::sqrt)?,
            Primitive::Sin => env.monadic_env(Value::sin)?,
            Primitive::Floor => env.monadic_env(Value::floor)?,
            Primitive::Ceil => env.monadic_env(Value::ceil)?,
            Primitive::Round => env.monadic_env(Value::round)?,
            Primitive::Eq => env.dyadic_oo_00_env(Value::is_eq)?,
            Primitive::Ne => env.dyadic_oo_00_env(Value::is_ne)?,
            Primitive::Lt => env.dyadic_oo_00_env(Value::is_lt)?,
            Primitive::Le => env.dyadic_oo_00_env(Value::is_le)?,
            Primitive::Gt => env.dyadic_oo_00_env(Value::is_gt)?,
            Primitive::Ge => env.dyadic_oo_00_env(Value::is_ge)?,
            Primitive::Add => env.dyadic_oo_00_env(Value::add)?,
            Primitive::Sub => env.dyadic_oo_00_env(Value::sub)?,
            Primitive::Mul => env.dyadic_oo_00_env(Value::mul)?,
            Primitive::Div => env.dyadic_oo_00_env(Value::div)?,
            Primitive::Mod => env.dyadic_oo_00_env(Value::modulus)?,
            Primitive::Pow => env.dyadic_oo_00_env(Value::pow)?,
            Primitive::Log => env.dyadic_oo_00_env(Value::log)?,
            Primitive::Min => env.dyadic_oo_00_env(Value::min)?,
            Primitive::Max => env.dyadic_oo_00_env(Value::max)?,
            Primitive::Atan => env.dyadic_oo_00_env(Value::atan2)?,
            Primitive::Complex => env.dyadic_oo_00_env(Value::complex)?,
            Primitive::Match => env.dyadic_rr(|a, b| a == b)?,
            Primitive::Join => env.dyadic_oo_env(Value::join)?,
            Primitive::Transpose => env.monadic_mut(Value::transpose)?,
            Primitive::Keep => env.dyadic_ro_env(Value::keep)?,
            Primitive::Take => env.dyadic_oo_env(Value::take)?,
            Primitive::Drop => env.dyadic_oo_env(Value::drop)?,
            Primitive::Rotate => env.dyadic_ro_env(Value::rotate)?,
            Primitive::Couple => env.dyadic_oo_env(Value::couple)?,
            Primitive::Rise => env.monadic_ref_env(|v, env| v.rise(env).map(Array::from))?,
            Primitive::Fall => env.monadic_ref_env(|v, env| v.fall(env).map(Array::from))?,
            Primitive::Pick => env.dyadic_oo_env(Value::pick)?,
            Primitive::Select => env.dyadic_rr_env(Value::select)?,
            Primitive::Windows => env.dyadic_rr_env(Value::windows)?,
            Primitive::Where => env.monadic_ref_env(Value::wher)?,
            Primitive::Classify => env.monadic_ref(Value::classify)?,
            Primitive::Deduplicate => env.monadic_mut(Value::deduplicate)?,
            Primitive::Unique => env.monadic_ref(Value::unique)?,
            Primitive::Member => env.dyadic_rr_env(Value::member)?,
            Primitive::Find => env.dyadic_rr_env(Value::find)?,
            Primitive::IndexOf => env.dyadic_rr_env(Value::index_of)?,
            // Primitive::ProgressiveIndexOf => env.dyadic_rr_env(Value::progressive_index_of)?,
            Primitive::Box => {
                let val = env.pop(1)?;
                env.push(Boxed(val));
            }
            Primitive::Parse => env.monadic_ref_env(Value::parse_num)?,
            Primitive::Utf => env.monadic_ref_env(Value::utf8)?,
            Primitive::Range => env.monadic_ref_env(Value::range)?,
            Primitive::Reverse => env.monadic_mut(Value::reverse)?,
            Primitive::Deshape => env.monadic_mut(Value::deshape)?,
            Primitive::Fix => env.monadic_mut(Value::fix)?,
            Primitive::First => env.monadic_env(Value::first)?,
            Primitive::Len => env.monadic_ref(Value::row_count)?,
            Primitive::Shape => env.monadic_ref(|v| {
                v.generic_ref(
                    Array::shape,
                    Array::shape,
                    Array::shape,
                    Array::shape,
                    Array::shape,
                )
                .iter()
                .copied()
                .collect::<Value>()
            })?,
            Primitive::Bits => env.monadic_ref_env(Value::bits)?,
            Primitive::Reduce => reduce::reduce(env)?,
            Primitive::Scan => reduce::scan(env)?,
            Primitive::Fold => reduce::fold(env)?,
            Primitive::Each => zip::each(env)?,
            Primitive::Rows => zip::rows(env)?,
            Primitive::Table | Primitive::Cross => table::table(env)?,
            Primitive::Repeat => loops::repeat(env)?,
            Primitive::Do => loops::do_(env)?,
            Primitive::Group => loops::group(env)?,
            Primitive::Partition => loops::partition(env)?,
            Primitive::Reshape => {
                let shape = env.pop(1)?;
                let mut array = env.pop(2)?;
                array.reshape(&shape, env)?;
                env.push(array);
            }
            Primitive::Rerank => {
                let rank = env.pop(1)?;
                let mut array = env.pop(2)?;
                array.rerank(&rank, env)?;
                env.push(array);
            }
            Primitive::Dup => {
                let x = env.pop(1)?;
                env.push(x.clone());
                env.push(x);
            }
            Primitive::Flip => {
                let a = env.pop(1)?;
                let b = env.pop(2)?;
                env.push(a);
                env.push(b);
            }
            Primitive::Over => {
                let a = env.pop(1)?;
                let b = env.pop(2)?;
                env.push(b.clone());
                env.push(a);
                env.push(b);
            }
            Primitive::Pop => {
                env.pop(1)?;
            }
            Primitive::Dip => {
                return Err(env.error("Dip was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Gap => {
                return Err(env.error("Gap was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Un => {
                return Err(env.error("Invert was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Under => {
                return Err(env.error("Under was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Bind => {
                return Err(env.error("Bind was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Unpack => {
                let f = env.pop_function()?;
                env.with_pack(|env| env.call(f))?;
            }
            Primitive::Content => {
                let f = env.pop_function()?;
                for val in env.stack_mut().iter_mut().rev().take(f.signature().args) {
                    val.unbox();
                }
                env.call(f)?;
            }
            Primitive::Fill => {
                let fill = env.pop_function()?;
                let f = env.pop_function()?;
                env.call(fill)?;
                let fill_value = env.pop("fill value")?;
                env.with_fill(fill_value, |env| env.call(f))?;
            }
            Primitive::Both => {
                return Err(env.error("Both was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Fork => {
                return Err(env.error("Fork was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Cascade => {
                return Err(env.error("Cascade was not inlined. This is a bug in the interpreter"))
            }
            Primitive::Bracket => {
                return Err(env.error("Bracket was not inlined. This is a bug in the interpreter"))
            }
            Primitive::All => algorithm::all(env)?,
            Primitive::This => {
                let f = env.pop_function()?;
                env.call_with_this(f)?;
            }
            Primitive::Recur => env.recur()?,
            Primitive::Try => {
                let f = env.pop_function()?;
                let handler = env.pop_function()?;
                let f_args = f.signature().args;
                let backup = env.clone_stack_top(f_args);
                if let Err(e) = env.call_clean_stack(f) {
                    env.rt
                        .backend
                        .save_error_color(e.message(), e.report().to_string());
                    env.push(e.value());
                    for val in backup {
                        env.push(val);
                    }
                    env.call(handler)?;
                }
            }
            Primitive::Assert => {
                let msg = env.pop(1)?;
                let cond = env.pop(2)?;
                if !cond.as_nat(env, "").is_ok_and(|n| n == 1) {
                    return Err(UiuaError::Throw(
                        msg.into(),
                        env.span().clone(),
                        env.inputs().clone().into(),
                    ));
                }
            }
            Primitive::Rand => env.push(random()),
            Primitive::Gen => {
                let seed = env.pop(1)?;
                let mut rng =
                    SmallRng::seed_from_u64(seed.as_num(env, "Gen expects a number")?.to_bits());
                let val: f64 = rng.gen();
                let next_seed = f64::from_bits(rng.gen::<u64>());
                env.push(val);
                env.push(next_seed);
            }
            Primitive::Deal => {
                let seed = env.pop(1)?.as_num(env, "Deal expects a number")?.to_bits();
                let arr = env.pop(2)?;
                let mut rows: Vec<Value> = arr.into_rows().collect();
                rows.shuffle(&mut SmallRng::seed_from_u64(seed));
                env.push(Value::from_row_values_infallible(rows));
            }
            Primitive::Tag => {
                static NEXT_TAG: AtomicUsize = AtomicUsize::new(0);
                let tag = NEXT_TAG.fetch_add(1, atomic::Ordering::Relaxed);
                env.push(tag);
            }
            Primitive::Type => {
                let val = env.pop(1)?;
                env.push(val.type_id());
            }
            Primitive::Memo => {
                let f = env.pop_function()?;
                let sig = f.signature();
                let mut args = Vec::with_capacity(sig.args);
                for i in 0..sig.args {
                    args.push(env.pop(i + 1)?);
                }
                let mut memo = env.rt.memo.get_or_default().borrow_mut();
                if let Some(f_memo) = memo.get_mut(&f.id) {
                    if let Some(outputs) = f_memo.get(&args) {
                        let outputs = outputs.clone();
                        drop(memo);
                        for val in outputs {
                            env.push(val);
                        }
                        return Ok(());
                    }
                }
                drop(memo);
                for arg in args.iter().rev() {
                    env.push(arg.clone());
                }
                let id = f.id.clone();
                env.call(f)?;
                let outputs = env.clone_stack_top(sig.outputs);
                let mut memo = env.rt.memo.get_or_default().borrow_mut();
                memo.borrow_mut()
                    .entry(id)
                    .or_default()
                    .insert(args, outputs.clone());
            }
            Primitive::Comptime => {
                return Err(env.error("Comptime was not inlined. This is a bug in the interpreter"));
            }
            Primitive::Spawn => {
                let f = env.pop_function()?;
                env.spawn(f.signature().args, |env| env.call(f))?;
            }
            Primitive::Wait => {
                let id = env.pop(1)?;
                env.wait(id)?;
            }
            Primitive::Send => {
                let val = env.pop(1)?;
                let id = env.pop(2)?;
                env.send(id, val)?;
            }
            Primitive::Recv => {
                let id = env.pop(1)?;
                env.recv(id)?;
            }
            Primitive::TryRecv => {
                let id = env.pop(1)?;
                env.try_recv(id)?;
            }
            Primitive::Now => env.push(instant::now() / 1000.0),
            Primitive::Rectify => {
                let f = env.pop_function()?;
                env.call(f)?;
            }
            Primitive::SetInverse => {
                let f = env.pop_function()?;
                let _inv = env.pop_function()?;
                env.call(f)?;
            }
            Primitive::SetUnder => {
                let f = env.pop_function()?;
                let _before = env.pop_function()?;
                let _after = env.pop_function()?;
                env.call(f)?;
            }
            Primitive::Insert => {
                let key = env.pop("key")?;
                let val = env.pop("value")?;
                let mut map = env.pop("map")?;
                map.insert(key, val, env)?;
                env.push(map);
            }
            Primitive::Has => {
                let key = env.pop("key")?;
                let map = env.pop("map")?;
                env.push(map.has_key(&key, env)?);
            }
            Primitive::Get => {
                let key = env.pop("key")?;
                let map = env.pop("map")?;
                let val = map.get(&key, env)?;
                env.push(val);
            }
            Primitive::Remove => {
                let key = env.pop("key")?;
                let mut map = env.pop("map")?;
                map.remove(key, env)?;
                env.push(map);
            }
            Primitive::Map => {
                let keys = env.pop("keys")?;
                let vals = env.pop("values")?;
                let map = keys.map(vals, env)?;
                env.push(map);
            }
            Primitive::Trace => trace(env, false)?,
            Primitive::Stack => stack(env, false)?,
            Primitive::Dump => dump(env, false)?,
            Primitive::Regex => regex(env)?,
            Primitive::Sys(io) => io.run(env)?,
        }
        Ok(())
    }
}

impl ImplPrimitive {
    pub(crate) fn run(&self, env: &mut Uiua) -> UiuaResult {
        match self {
            ImplPrimitive::Asin => env.monadic_env(Value::asin)?,
            ImplPrimitive::Acos => env.monadic_env(Value::acos)?,
            ImplPrimitive::Unkeep => {
                let from = env.pop(1)?;
                let counts = env.pop(2)?;
                let into = env.pop(3)?;
                env.push(from.unkeep(counts, into, env)?);
            }
            ImplPrimitive::Untake => {
                let index = env.pop(1)?;
                let into = env.pop(2)?;
                let from = env.pop(3)?;
                env.push(from.untake(index, into, env)?);
            }
            ImplPrimitive::Undrop => {
                let index = env.pop(1)?;
                let into = env.pop(2)?;
                let from = env.pop(3)?;
                env.push(from.undrop(index, into, env)?);
            }
            ImplPrimitive::InvCouple => {
                let coupled = env.pop(1)?;
                let (a, b) = coupled.uncouple(env)?;
                env.push(b);
                env.push(a);
            }
            ImplPrimitive::InvMap => {
                let map = env.pop(1)?;
                let (keys, vals) = map.unmap(env)?;
                env.push(vals);
                env.push(keys);
            }
            ImplPrimitive::Unpick => {
                let index = env.pop(1)?;
                let into = env.pop(2)?;
                let from = env.pop(3)?;
                env.push(from.unpick(index, into, env)?);
            }
            ImplPrimitive::Unselect => {
                let index = env.pop(1)?;
                let into = env.pop(2)?;
                let from = env.pop(3)?;
                env.push(from.unselect(index, into, env)?);
            }
            ImplPrimitive::Unrerank => {
                let rank = env.pop(1)?;
                let shape = env.pop(2)?;
                let mut array = env.pop(3)?;
                array.unrerank(&rank, &shape, env)?;
                env.push(array);
            }
            ImplPrimitive::Unreshape => {
                let orig_shape = env.pop(1)?;
                let mut array = env.pop(2)?;
                array.unreshape(&orig_shape, env)?;
                env.push(array);
            }
            ImplPrimitive::Unfirst => {
                let into = env.pop(1)?;
                let from = env.pop(2)?;
                env.push(from.unfirst(into, env)?);
            }
            ImplPrimitive::Unlast => {
                let into = env.pop(1)?;
                let from = env.pop(2)?;
                env.push(from.unlast(into, env)?);
            }
            ImplPrimitive::InvWhere => env.monadic_ref_env(Value::inverse_where)?,
            ImplPrimitive::InvUtf => env.monadic_ref_env(Value::inv_utf8)?,
            ImplPrimitive::InverseBits => env.monadic_ref_env(Value::inv_bits)?,
            ImplPrimitive::Unpartition => loops::unpartition(env)?,
            ImplPrimitive::Ungroup => loops::ungroup(env)?,
            ImplPrimitive::Unjoin => {
                let b_rank = env.pop(1)?;
                let a_rank = env.pop(2)?;
                let val = env.pop(3)?;
                let (left, right) = val.unjoin(a_rank, b_rank, env)?;
                env.push(right);
                env.push(left);
            }
            ImplPrimitive::InvAtan => {
                let x = env.pop(1)?;
                let sin = x.clone().sin(env)?;
                let cos = x.cos(env)?;
                env.push(cos);
                env.push(sin);
            }
            ImplPrimitive::InvComplex => {
                let x = env.pop(1)?;
                let im = x.clone().complex_im(env)?;
                let re = x.complex_re(env)?;
                env.push(re);
                env.push(im);
            }
            ImplPrimitive::InvParse => env.monadic_ref_env(Value::inv_parse)?,
            ImplPrimitive::InvFix => env.monadic_mut(Value::inv_fix)?,
            ImplPrimitive::InvScan => reduce::invscan(env)?,
            ImplPrimitive::InvTrace => trace(env, true)?,
            ImplPrimitive::InvStack => stack(env, true)?,
            ImplPrimitive::InvDump => dump(env, true)?,
            ImplPrimitive::Primes => env.monadic_ref_env(Value::primes)?,
            ImplPrimitive::InvBox => {
                let val = env.pop(1)?;
                env.push(val.unboxed());
            }
            // Optimizations
            ImplPrimitive::Cos => env.monadic_env(Value::cos)?,
            ImplPrimitive::Last => env.monadic_env(Value::last)?,
            ImplPrimitive::FirstMinIndex => env.monadic_ref_env(Value::first_min_index)?,
            ImplPrimitive::FirstMaxIndex => env.monadic_ref_env(Value::first_max_index)?,
            ImplPrimitive::LastMinIndex => env.monadic_ref_env(Value::last_min_index)?,
            ImplPrimitive::LastMaxIndex => env.monadic_ref_env(Value::last_max_index)?,
            ImplPrimitive::FirstWhere => env.monadic_ref_env(Value::first_where)?,
            ImplPrimitive::SortUp => env.monadic_mut_env(Value::sort_up)?,
            ImplPrimitive::SortDown => env.monadic_mut_env(Value::sort_down)?,
            ImplPrimitive::ReduceContent => reduce::reduce_content(env)?,
            ImplPrimitive::ReplaceRand => {
                env.pop(1)?;
                env.push(random());
            }
            ImplPrimitive::ReplaceRand2 => {
                env.pop(1)?;
                env.pop(2)?;
                env.push(random());
            }
            &ImplPrimitive::TransposeN(n) => env.monadic_mut(|val| val.transpose_depth(0, n))?,
        }
        Ok(())
    }
}

#[cfg(not(feature = "regex"))]
fn regex(env: &mut Uiua) -> UiuaResult {
    Err(env.error("Regex support is not enabled"))
}

#[cfg(feature = "regex")]
fn regex(env: &mut Uiua) -> UiuaResult {
    use std::collections::HashMap;

    use ecow::EcoVec;
    use regex::Regex;

    thread_local! {
        pub static REGEX_CACHE: RefCell<HashMap<String, Regex>> = RefCell::new(HashMap::new());
    }
    let pattern = env.pop(1)?.as_string(env, "Pattern must be a string")?;
    let target = env
        .pop(1)?
        .as_string(env, "Matching target must be a string")?;
    REGEX_CACHE.with(|cache| -> UiuaResult {
        let mut cache = cache.borrow_mut();
        let regex = if let Some(regex) = cache.get(&pattern) {
            regex
        } else {
            let regex =
                Regex::new(&pattern).map_err(|e| env.error(format!("Invalid pattern: {}", e)))?;
            cache.entry(pattern.clone()).or_insert(regex.clone())
        };

        let mut matches: Value =
            Array::<Boxed>::new([0, regex.captures_len()].as_slice(), []).into();

        for caps in regex.captures_iter(&target) {
            let row: EcoVec<Boxed> = caps
                .iter()
                .flat_map(|m| {
                    m.map(|m| Boxed(Value::from(m.as_str())))
                        .or_else(|| env.value_fill().cloned().map(Value::boxed_if_not))
                })
                .collect();
            matches.append(row.into(), env)?;
        }

        env.push(matches);
        Ok(())
    })
}

/// Generate a random number, equivalent to [`Primitive::Rand`]
pub fn random() -> f64 {
    thread_local! {
        static RNG: RefCell<SmallRng> = RefCell::new(SmallRng::seed_from_u64(instant::now().to_bits()));
    }
    RNG.with(|rng| rng.borrow_mut().gen::<f64>())
}

fn trace(env: &mut Uiua, inverse: bool) -> UiuaResult {
    let val = env.pop(1)?;
    let span: String = if inverse {
        format!("{}{} {}", Primitive::Un, Primitive::Trace, env.span())
    } else {
        env.span().to_string()
    };
    let max_line_len = span.chars().count() + 2;
    let item_lines =
        format_trace_item_lines(val.show().lines().map(Into::into).collect(), max_line_len);
    env.push(val);
    env.rt.backend.print_str_trace(&format!("┌╴{span}\n"));
    for line in item_lines {
        env.rt.backend.print_str_trace(&line);
    }
    env.rt.backend.print_str_trace("└");
    for _ in 0..max_line_len - 1 {
        env.rt.backend.print_str_trace("╴");
    }
    env.rt.backend.print_str_trace("\n");
    Ok(())
}

fn stack(env: &Uiua, inverse: bool) -> UiuaResult {
    let span = if inverse {
        format!("{}{} {}", Primitive::Un, Primitive::Stack, env.span())
    } else {
        format!("{} {}", Primitive::Stack, env.span())
    };
    let items = env.clone_stack_top(env.stack_height());
    let max_line_len = span.chars().count() + 2;
    let boundaries = stack_boundaries(env);
    let item_lines: Vec<Vec<String>> = items
        .iter()
        .map(Value::show)
        .map(|s| s.lines().map(Into::into).collect::<Vec<String>>())
        .map(|lines| format_trace_item_lines(lines, max_line_len))
        .enumerate()
        .flat_map(|(i, lines)| {
            if let Some((_, id)) = boundaries.iter().find(|(height, _)| i == *height) {
                vec![vec![format!("│╴╴╴{id}╶╶╶\n")], lines]
            } else {
                vec![lines]
            }
        })
        .collect();
    env.rt.backend.print_str_trace(&format!("┌╴{span}\n"));
    for line in item_lines.iter().flatten() {
        env.rt.backend.print_str_trace(line);
    }
    env.rt.backend.print_str_trace("└");
    for _ in 0..max_line_len - 1 {
        env.rt.backend.print_str_trace("╴");
    }
    env.rt.backend.print_str_trace("\n");
    Ok(())
}

fn dump(env: &mut Uiua, inverse: bool) -> UiuaResult {
    let f = env.pop_function()?;
    if f.signature() != (1, 1) {
        return Err(env.error(format!(
            "Dump's function's signature must be |1.1, but it is {}",
            f.signature()
        )));
    }
    let span = if inverse {
        format!("{}{} {}", Primitive::Un, Primitive::Dump, env.span())
    } else {
        format!("{} {}", Primitive::Dump, env.span())
    };
    let unprocessed = env.clone_stack_top(env.stack_height());
    let mut items = Vec::new();
    for item in unprocessed {
        env.push(item);
        match env.call(f.clone()) {
            Ok(()) => items.push(env.pop("dump's function's processed result")?),
            Err(e) => items.push(e.value()),
        }
    }
    let max_line_len = span.chars().count() + 2;
    let boundaries = stack_boundaries(env);
    let item_lines: Vec<Vec<String>> = items
        .iter()
        .map(Value::show)
        .map(|s| s.lines().map(Into::into).collect::<Vec<String>>())
        .map(|lines| format_trace_item_lines(lines, max_line_len))
        .enumerate()
        .flat_map(|(i, lines)| {
            if let Some((_, id)) = boundaries.iter().find(|(height, _)| i == *height) {
                vec![vec![format!("│╴╴╴{id}╶╶╶\n")], lines]
            } else {
                vec![lines]
            }
        })
        .collect();
    env.rt.backend.print_str_trace(&format!("┌╴{span}\n"));
    for line in item_lines.iter().flatten() {
        env.rt.backend.print_str_trace(line);
    }
    env.rt.backend.print_str_trace("└");
    for _ in 0..max_line_len - 1 {
        env.rt.backend.print_str_trace("╴");
    }
    env.rt.backend.print_str_trace("\n");
    Ok(())
}

fn stack_boundaries(env: &Uiua) -> Vec<(usize, &FunctionId)> {
    let mut boundaries: Vec<(usize, &FunctionId)> = Vec::new();
    let mut height = 0;
    let mut reduced = 0;
    for (i, frame) in env.call_frames().rev().enumerate() {
        if i == 0 {
            let before_sig = instrs_signature(&env.instrs(frame.slice)[..frame.pc])
                .ok()
                .unwrap_or(frame.sig);
            reduced = before_sig.args as isize - before_sig.outputs as isize;
        }
        height = height.max(((frame.sig.args as isize) - reduced).max(0) as usize);
        if matches!(frame.id, FunctionId::Main) {
            break;
        }
        boundaries.push((env.stack_height().saturating_sub(height), &frame.id));
    }
    boundaries
}

fn format_trace_item_lines(mut lines: Vec<String>, mut max_line_len: usize) -> Vec<String> {
    let lines_len = lines.len();
    for (j, line) in lines.iter_mut().enumerate() {
        let stick = if lines_len == 1 || j == 1 {
            "├╴"
        } else {
            "│ "
        };
        line.insert_str(0, stick);
        max_line_len = max_line_len.max(line.chars().count());
        line.push('\n');
    }
    lines
}

/// Documentation for a primitive
#[derive(Default, Debug)]
pub struct PrimDoc {
    /// The short description
    pub short: Vec<PrimDocFragment>,
    /// The full documentation
    pub lines: Vec<PrimDocLine>,
}

impl PrimDoc {
    /// Get the primitive's short description
    pub fn short_text(&self) -> Cow<str> {
        if self.short.len() == 1 {
            match &self.short[0] {
                PrimDocFragment::Text(t) => return Cow::Borrowed(t),
                PrimDocFragment::Code(c) => return Cow::Borrowed(c),
                PrimDocFragment::Emphasis(e) => return Cow::Borrowed(e),
                PrimDocFragment::Strong(s) => return Cow::Borrowed(s),
                PrimDocFragment::Primitive { prim, named: true } => {
                    return Cow::Borrowed(prim.name());
                }
                PrimDocFragment::Link { text, .. } => return Cow::Borrowed(text),
                PrimDocFragment::Primitive { .. } => {}
            }
        }
        let mut s = String::new();
        for frag in &self.short {
            match frag {
                PrimDocFragment::Text(t) => s.push_str(t),
                PrimDocFragment::Code(c) => s.push_str(c),
                PrimDocFragment::Emphasis(e) => s.push_str(e),
                PrimDocFragment::Strong(str) => s.push_str(str),
                PrimDocFragment::Link { text, .. } => s.push_str(text),
                PrimDocFragment::Primitive { prim, named } => {
                    if *named {
                        s.push_str(prim.name());
                    } else if let Some(c) = prim.glyph() {
                        s.push(c);
                    } else {
                        s.push_str(prim.name());
                    }
                }
            }
        }
        Cow::Owned(s)
    }
    pub(crate) fn from_lines(s: &str) -> Self {
        let mut short = Vec::new();
        let mut lines = Vec::new();
        for line in s.lines() {
            let line = line.trim();
            if let Some(mut ex) = line.strip_prefix("ex:") {
                // Example
                if ex.starts_with(' ') {
                    ex = &ex[1..]
                }
                lines.push(PrimDocLine::Example(PrimExample {
                    input: ex.into(),
                    should_error: false,
                    output: OnceLock::new(),
                }));
            } else if let Some(mut ex) = line.strip_prefix("ex!") {
                // Example
                if ex.starts_with(' ') {
                    ex = &ex[1..]
                }
                lines.push(PrimDocLine::Example(PrimExample {
                    input: ex.into(),
                    should_error: true,
                    output: OnceLock::new(),
                }));
            } else if let Some(mut ex) = line.strip_prefix(':') {
                // Continue example
                if ex.starts_with(' ') {
                    ex = &ex[1..]
                }
                if let Some(PrimDocLine::Example(example)) = lines.last_mut() {
                    example.input.push('\n');
                    example.input.push_str(ex);
                } else {
                    lines.push(PrimDocLine::Text(parse_doc_line_fragments(line)));
                }
            } else if short.is_empty() {
                // Set short
                short = parse_doc_line_fragments(line);
            } else {
                // Add line
                lines.push(PrimDocLine::Text(parse_doc_line_fragments(line)));
            }
        }
        while let Some(PrimDocLine::Text(frags)) = lines.first() {
            if frags.is_empty() {
                lines.remove(0);
            } else {
                break;
            }
        }
        while let Some(PrimDocLine::Text(frags)) = lines.last() {
            if frags.is_empty() {
                lines.pop();
            } else {
                break;
            }
        }
        Self { short, lines }
    }
}

/// An primitive code example
#[derive(Debug)]
pub struct PrimExample {
    input: String,
    should_error: bool,
    output: OnceLock<Result<Vec<String>, String>>,
}

impl PrimExample {
    /// Get the example's source code
    pub fn input(&self) -> &str {
        &self.input
    }
    /// Check whether the example should error
    pub fn should_error(&self) -> bool {
        self.should_error
    }
    /// Check whether the example should run automatically in certain contexts
    pub fn should_run(&self) -> bool {
        !["&sl", "&tcpc", "&ast", "&p", "&fwa"]
            .iter()
            .any(|prim| self.input.contains(prim))
    }
    /// Get the example's output
    pub fn output(&self) -> &Result<Vec<String>, String> {
        self.output.get_or_init(|| {
            let mut env = Uiua::with_safe_sys();
            match env.run_str(&self.input) {
                Ok(_) => Ok(env.take_stack().into_iter().map(|val| val.show()).collect()),
                Err(e) => Err(e
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .split_once(' ')
                    .unwrap_or_default()
                    .1
                    .into()),
            }
        })
    }
}

/// A line in a primitive's documentation
#[derive(Debug)]
pub enum PrimDocLine {
    /// Just text
    Text(Vec<PrimDocFragment>),
    /// An example
    Example(PrimExample),
}

/// A pseudo-markdown fragment for primitive documentation
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub enum PrimDocFragment {
    Text(String),
    Code(String),
    Emphasis(String),
    Strong(String),
    Primitive { prim: Primitive, named: bool },
    Link { text: String, url: String },
}

fn parse_doc_line_fragments(line: &str) -> Vec<PrimDocFragment> {
    let mut frags = Vec::new();
    #[derive(PartialEq, Eq)]
    enum FragKind {
        Text,
        Code,
        Emphasis,
        Strong,
        Primitive,
    }
    impl FragKind {
        fn open(&self) -> &str {
            match self {
                FragKind::Text => "",
                FragKind::Code => "`",
                FragKind::Emphasis => "*",
                FragKind::Strong => "**",
                FragKind::Primitive => "[",
            }
        }
    }
    let mut curr = String::new();
    let mut kind = FragKind::Text;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'`') => {
                curr.push('`');
                chars.next();
            }
            '`' if kind == FragKind::Code => {
                if let Some(prim) = Primitive::from_name(&curr) {
                    frags.push(PrimDocFragment::Primitive { prim, named: false });
                } else {
                    frags.push(PrimDocFragment::Code(curr));
                }
                curr = String::new();
                kind = FragKind::Text;
            }
            '`' if kind == FragKind::Text => {
                frags.push(PrimDocFragment::Text(curr));
                curr = String::new();
                kind = FragKind::Code;
            }
            '*' if kind == FragKind::Emphasis && curr.is_empty() => {
                kind = FragKind::Strong;
            }
            '*' if kind == FragKind::Emphasis => {
                frags.push(PrimDocFragment::Emphasis(curr));
                curr = String::new();
                kind = FragKind::Text;
            }
            '*' if kind == FragKind::Strong && chars.peek() == Some(&'*') => {
                chars.next();
                frags.push(PrimDocFragment::Strong(curr));
                curr = String::new();
                kind = FragKind::Text;
            }
            '*' if kind == FragKind::Text => {
                frags.push(PrimDocFragment::Text(curr));
                curr = String::new();
                kind = FragKind::Emphasis;
            }
            '[' if kind == FragKind::Text => {
                frags.push(PrimDocFragment::Text(curr));
                curr = String::new();
                kind = FragKind::Primitive;
            }
            ']' if kind == FragKind::Primitive && chars.peek() == Some(&'(') => {
                chars.next();
                let mut url = String::new();
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                    url.push(c);
                }
                frags.push(PrimDocFragment::Link {
                    text: curr,
                    url: url.trim().to_owned(),
                });
                curr = String::new();
                kind = FragKind::Text;
            }
            ']' if kind == FragKind::Primitive => {
                if let Some(prim) = Primitive::from_name(&curr) {
                    frags.push(PrimDocFragment::Primitive { prim, named: true });
                } else {
                    frags.push(PrimDocFragment::Text(curr));
                }
                curr = String::new();
                kind = FragKind::Text;
            }
            ']' if kind == FragKind::Text => {
                frags.push(PrimDocFragment::Text(curr));
                curr = String::new();
            }
            c => curr.push(c),
        }
    }
    curr.insert_str(0, kind.open());
    if !curr.is_empty() {
        frags.push(PrimDocFragment::Text(curr));
    }
    frags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_collisions() {
        for a in Primitive::all() {
            for b in Primitive::all() {
                if a >= b {
                    continue;
                }
                assert_ne!(a.name(), b.name(), "{a:?} and {b:?} have the same name",)
            }
        }
    }

    #[test]
    #[cfg(feature = "native_sys")]
    fn prim_docs() {
        for prim in Primitive::non_deprecated() {
            for line in &prim.doc().lines {
                if let PrimDocLine::Example(ex) = line {
                    if !ex.should_run() {
                        continue;
                    }
                    println!("{prim} example:\n{}", ex.input);
                    match Uiua::with_safe_sys().run_str(&ex.input) {
                        Ok(mut comp) => {
                            if let Some(diag) = comp.take_diagnostics().into_iter().next() {
                                if !ex.should_error {
                                    panic!("\nExample failed:\n{}\n{}", ex.input, diag.report());
                                }
                            } else if ex.should_error {
                                panic!("Example should have failed: {}", ex.input);
                            }
                        }
                        Err(e) => {
                            if !ex.should_error {
                                panic!("\nExample failed:\n{}\n{}", ex.input, e.report());
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn primitive_from_name() {
        for prim in Primitive::non_deprecated() {
            assert_eq!(Primitive::from_name(prim.name()), Some(prim));
        }
        for (name, test) in [
            (
                "from_format_name",
                Primitive::from_format_name as fn(&str) -> Option<Primitive>,
            ),
            ("from_format_name_multi", |name| {
                Primitive::from_format_name_multi(name)
                    .unwrap()
                    .first()
                    .map(|(prim, _)| *prim)
            }),
        ] {
            for prim in Primitive::non_deprecated() {
                let char_test = match prim.glyph() {
                    None => prim.name().len(),
                    Some(c) if c.is_ascii() => continue,
                    Some(_) => 4,
                };
                let short: String = prim.name().chars().take(char_test).collect();
                assert_eq!(test(&short), Some(prim));
            }
            for prim in Primitive::non_deprecated() {
                if matches!(
                    prim,
                    Primitive::Rand
                        | Primitive::Trace
                        | Primitive::Rectify
                        | Primitive::Recur
                        | Primitive::Parse
                ) {
                    continue;
                }
                let char_test = match prim.glyph() {
                    None => prim.name().len(),
                    Some(c) if c.is_ascii() || prim.ascii().is_some() => continue,
                    Some(_) => 3,
                };
                let short: String = prim.name().chars().take(char_test).collect();
                assert_eq!(
                    test(&short),
                    Some(prim),
                    "{} does not format from {:?} with {}",
                    prim.format(),
                    short,
                    name
                );
            }
        }
        assert_eq!(Primitive::from_format_name("id"), Some(Primitive::Identity));
    }

    #[test]
    fn from_multiname() {
        assert!(matches!(
            &*Primitive::from_format_name_multi("rev").expect("rev"),
            [(Primitive::Reverse, _)]
        ));
        assert!(matches!(
            &*Primitive::from_format_name_multi("revrev").expect("revrev"),
            [(Primitive::Reverse, _), (Primitive::Reverse, _)]
        ));
        assert!(matches!(
            &*Primitive::from_format_name_multi("tabkee").unwrap(),
            [(Primitive::Table, _), (Primitive::Keep, _)]
        ));
        assert_eq!(Primitive::from_format_name_multi("foo"), None);
    }

    #[cfg(test)]
    #[test]
    fn gen_grammar_file() {
        fn gen_group(prims: impl Iterator<Item = Primitive> + Clone) -> String {
            let glyphs = prims
                .clone()
                .flat_map(|p| {
                    p.glyph()
                        .into_iter()
                        .chain(p.ascii().into_iter().flat_map(|ascii| {
                            Some(ascii.to_string())
                                .filter(|s| s.len() == 1)
                                .into_iter()
                                .flat_map(|s| s.chars().collect::<Vec<_>>())
                        }))
                })
                .collect::<String>()
                .replace('\\', "\\\\\\\\")
                .replace('-', "\\\\-")
                .replace('*', "\\\\*")
                .replace('^', "\\\\^");
            let format_names: Vec<_> = prims
                .clone()
                .map(|p| {
                    let name = p.name();
                    let min_len = if name.starts_with('&') {
                        name.len()
                    } else {
                        (2..=name.len())
                            .find(|&n| Primitive::from_format_name(&name[..n]) == Some(p))
                            .unwrap()
                    };
                    let mut start: String = name.chars().take(min_len).collect();
                    let mut end = String::new();
                    for c in name.chars().skip(min_len) {
                        start.push('(');
                        start.push(c);
                        end.push_str(")?");
                    }
                    format!("{}{}", start, end)
                })
                .collect();
            let format_names = format_names.join("|");
            let mut literal_names: Vec<_> = prims
                .map(|p| p.names())
                .filter(|p| p.ascii.is_none() && p.glyph.is_none())
                .map(|n| format!("|{}", n.text))
                .collect();
            literal_names.sort_by_key(|s| s.len());
            literal_names.reverse();
            let literal_names = literal_names.join("");
            format!(r#"[{glyphs}]|(?<![a-zA-Z])({format_names}{literal_names})(?![a-zA-Z])"#)
        }

        let stack_functions = gen_group(
            Primitive::non_deprecated()
                .filter(|p| p.class() == PrimClass::Stack && p.modifier_args().is_none())
                .chain(Some(Primitive::Identity)),
        );
        let noadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            p.class() != PrimClass::Stack && p.modifier_args().is_none() && p.args() == Some(0)
        }));
        let monadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            ![PrimClass::Stack, PrimClass::Planet].contains(&p.class())
                && p.modifier_args().is_none()
                && p.args() == Some(1)
        }));
        let dyadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            p.class() != PrimClass::Stack && p.modifier_args().is_none() && p.args() == Some(2)
        }));
        let monadic_modifiers =
            gen_group(Primitive::non_deprecated().filter(|p| matches!(p.modifier_args(), Some(1))));
        let dyadic_modifiers: String = gen_group(
            Primitive::non_deprecated().filter(|p| matches!(p.modifier_args(), Some(n) if n >= 2)),
        );

        let text = format!(
            r##"{{
	"$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
	"name": "Uiua",
	"patterns": [
		{{
			"include": "#comments"
		}},
		{{
			"include": "#strings-multiline"
		}},
		{{
			"include": "#strings-format"
		}},
		{{
			"include": "#strings-normal"
		}},
        {{
            "include": "#characters"
        }},
		{{
			"include": "#numbers"
		}},
        {{
            "include": "#strand"
        }},
		{{
			"include": "#stack"
		}},
		{{
			"include": "#noadic"
		}},
		{{
			"include": "#monadic"
		}},
		{{
			"include": "#dyadic"
		}},
		{{
			"include": "#mod1"
		}},
		{{
			"include": "#mod2"
		}},
        {{
            "include": "#idents"
        }}
	],
	"repository": {{
        "idents": {{
            "name": "variable.parameter.uiua",
            "match": "\\b[a-zA-Z]+\\b"
        }},
		"comments": {{
			"name": "comment.line.uiua",
			"match": "#.*$"
		}},
		"strings-normal": {{
			"name": "constant.character.escape",
			"begin": "\"",
			"end": "\"",
			"patterns": [
				{{
					"name": "string.quoted",
					"match": "\\\\[\\\\\"0nrt]"
				}}
			]
		}},
		"strings-format": {{
			"name": "constant.character.escape",
			"begin": "\\$\"",
			"end": "\"",
			"patterns": [
				{{
					"name": "string.quoted",
					"match": "\\\\[\\\\\"0nrt_]"
				}},
				{{
					"name": "constant.numeric",
					"match": "(?<!\\\\)_"
				}}
			]
		}},
		"strings-multiline": {{
			"name": "constant.character.escape",
			"begin": "\\$ ",
			"end": "$",
			"patterns": [
				{{
					"name": "string.quoted",
					"match": "\\\\[\\\\\"0nrt_]"
				}},
				{{
					"name": "constant.numeric",
					"match": "(?<!\\\\)_"
				}}
			]
		}},
        "characters": {{
            "name": "constant.character.escape",
            "match": "@(\\\\(x[0-9A-Fa-f]{{2}}|u[0-9A-Fa-f]{{4}}|.)|.)"
        }},
		"numbers": {{
			"name": "constant.numeric.uiua",
			"match": "[`¯]?\\d+([./]\\d+)?(e[+-]?\\d+)?"
		}},
		"strand": {{
			"name": "comment.line",
			"match": "_"
		}},
        "stack": {{
            "match": "{stack_functions}"
        }},
		"noadic": {{
			"name": "entity.name.tag.uiua",
            "match": "{noadic_functions}"
        }},
		"monadic": {{
			"name": "string.quoted",
            "match": "{monadic_functions}"
        }},
		"dyadic": {{
			"name": "entity.name.function.uiua",
            "match": "{dyadic_functions}"
        }},
		"mod1": {{
			"name": "entity.name.type.uiua",
            "match": "{monadic_modifiers}"
        }},
		"mod2": {{
			"name": "keyword.control.uiua",
            "match": "{dyadic_modifiers}"
        }}
    }},
	"scopeName": "source.uiua"
}}"##
        );

        std::fs::write("uiua.tmLanguage.json", text).expect("Failed to write grammar file");
    }

    #[test]
    fn gen_syntax_file() {
        fn gen_group(prims: impl Iterator<Item = Primitive> + Clone) -> String {
            let glyphs = prims
                .clone()
                .flat_map(|p| {
                    p.glyph()
                        .into_iter()
                        .chain(p.ascii().into_iter().flat_map(|ascii| {
                            Some(ascii.to_string())
                                .filter(|s| s.len() == 1)
                                .into_iter()
                                .flat_map(|s| s.chars().collect::<Vec<_>>())
                        }))
                })
                .collect::<String>()
                .replace('-', r#"\-"#);
            let sys_names: Vec<_> = prims
                .clone()
                .map(|p| p.name())
                .filter_map(|n| n.strip_prefix('&'))
                .collect();
            let sys_names = if sys_names.len() > 0 {
                format!(r#"\|&\%({}\)"#, sys_names.join(r#"\|"#))
            } else {
                "".into()
            };

            let format_names: String = prims
                .clone()
                .filter_map(|p| {
                    let name = p.name();
                    if name.starts_with('&') {
                        None
                    } else {
                        let min_len = (2..=name.len())
                            .find(|&n| Primitive::from_format_name(&name[..n]) == Some(p))
                            .unwrap();
                        let mut start: String = name.chars().take(min_len).collect();
                        let mut end = String::new();
                        for c in name.chars().skip(min_len) {
                            start.push_str(r#"\%("#);
                            start.push(c);
                            end.push_str(r#"\)\?"#);
                        }
                        Some(format!("{}{}", start, end))
                    }
                })
                .collect::<Vec<_>>()
                .join("\\|");
            let format_names = format!(r#"\%({}\)"#, format_names);

            // let mut literal_names: Vec<_> = prims
            //     .map(|p| p.names())
            //     .filter(|p| p.ascii.is_none() && p.glyph.is_none())
            //     .map(|n| format!("|{}", n.text))
            //     .collect();
            // literal_names.sort_by_key(|s| s.len());
            // literal_names.reverse();
            // let literal_names = literal_names.join("");
            format!(r#"[{glyphs}]\|\([a-zA-Z]\)\@<!\({format_names}{sys_names}\)\([a-zA-Z]\)\@!"#)
        }

        let stack_functions = gen_group(
            Primitive::non_deprecated()
                .filter(|p| p.class() == PrimClass::Stack && p.modifier_args().is_none())
                .chain(Some(Primitive::Identity)),
        );
        let noadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            p.class() != PrimClass::Stack && p.modifier_args().is_none() && p.args() == Some(0)
        }));
        let monadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            ![PrimClass::Stack, PrimClass::Planet].contains(&p.class())
                && p.modifier_args().is_none()
                && p.args() == Some(1)
        }));
        let dyadic_functions = gen_group(Primitive::non_deprecated().filter(|p| {
            p.class() != PrimClass::Stack && p.modifier_args().is_none() && p.args() == Some(2)
        }));
        let monadic_modifiers =
            gen_group(Primitive::non_deprecated().filter(|p| matches!(p.modifier_args(), Some(1))));
        let dyadic_modifiers: String = gen_group(
            Primitive::non_deprecated().filter(|p| matches!(p.modifier_args(), Some(n) if n >= 2)),
        );

        let text = format!(
            r##"if exists('b:current_syntax')
    finish
endif

syn match uiuaidents "\<[a-zA-Z]\+\>"
syn match uiuacomments "#.*$"
syn match uiuacharacters "@\%(\\\%(x[0-9A-Fa-f]{{2}}\|u[0-9A-Fa-f]{{4}}\|.\)\|.\)"
syn match uiuanumbers "[`¯]\?[0-9]\+\%([./][0-9]\+\%(e[+-]\?[0-9]\+\)\?\)\?"
syn match uiuastrand "_"
syn match uiuastack "{stack_functions}"
syn match uiuanoadic "{noadic_functions}"
syn match uiuamonadic "{monadic_functions}"
syn match uiuadyadic "{dyadic_functions}"
syn match uiuamod1 "{monadic_modifiers}"
syn match uiuamod2 "{dyadic_modifiers}"

syn match uiuaquote /""/ contained
syn region uiuastringsnormal matchgroup=uiuastringsnormal start=/"/ end=/"/ contains=uiuaquote
syn region uiuastringsformat matchgroup=uiuastringsformat start=/\$"/ end=/"/ contains=uiuaquote
syn region uiuastringsmultil matchgroup=uiuastringsmultil start=/\$ / end=/$/ contains=uiuaquote
syn sync fromstart

hi link uiuaidents Identifier
hi link uiuacomments Comment
hi link uiuacharacters Character
hi link uiuanumbers Number
hi link uiuastringsnormal String
hi link uiuastringsformat String
hi link uiuastringsmultil String
hi link uiuastrand Comment
hi link uiuastack Normal
hi link uiuanoadic Special
hi link uiuamonadic Macro
hi link uiuadyadic Function
hi link uiuamod1 Type
hi link uiuamod2 Keyword

if has ('nvim')
    hi link @lsp.type.string String
    hi link @lsp.type.number Number
    hi link @lsp.type.comment Comment
    hi link @lsp.type.stack_function Normal
    hi link @lsp.type.noadic_function Special
    hi link @lsp.type.monadic_function Macro
    hi link @lsp.type.dyadic_function Function
    hi link @lsp.type.monadic_modifier Type
    hi link @lsp.type.dyadic_modifier Keyword
endif

let b:current_syntax='uiua'
"##);

        std::fs::write("uiua.vim", text).expect("Failed to write syntax file");
    }
}
