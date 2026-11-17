//! Source-bound probe correction proof. Parsing and evaluation are native Rust;
//! no submitted code, Python interpreter, imports or product calls are executed.
use ast::bigint::BigInt;
use rustpython_ast::{
    self as ast,
    fold::{self, Fold},
    text_size::TextRange,
};
use rustpython_parser::Parse;
use std::{cmp::Ordering, collections::BTreeMap};

type Tree = Vec<ast::Stmt<()>>;
#[derive(Default)]
struct Normalize {
    nodes: usize,
    depth: usize,
    re_import: bool,
    test_import: bool,
    patterns: Vec<String>,
    rewrite: Option<(usize, String)>,
}
impl Fold<TextRange> for Normalize {
    type TargetU = ();
    type Error = ();
    type UserContext = ();
    fn will_map_user(&mut self, _: &TextRange) {}
    fn map_user(&mut self, _: TextRange, _: ()) -> Result<(), ()> {
        self.nodes += 1;
        Ok(())
    }
    fn fold_expr(&mut self, node: ast::Expr) -> Result<ast::Expr<()>, ()> {
        self.depth += 1;
        if self.depth > 200 {
            return Err(());
        }
        let result = fold::fold_expr(self, node);
        self.depth -= 1;
        result
    }
    fn fold_expr_context(&mut self, node: ast::ExprContext) -> Result<ast::ExprContext, ()> {
        self.nodes += 1;
        Ok(node)
    }
    fn fold_cmpop(&mut self, node: ast::CmpOp) -> Result<ast::CmpOp, ()> {
        self.nodes += 1;
        Ok(node)
    }
    fn fold_unaryop(&mut self, node: ast::UnaryOp) -> Result<ast::UnaryOp, ()> {
        self.nodes += 1;
        Ok(node)
    }
    fn fold_operator(&mut self, node: ast::Operator) -> Result<ast::Operator, ()> {
        self.nodes += 1;
        Ok(node)
    }
    fn fold_boolop(&mut self, node: ast::BoolOp) -> Result<ast::BoolOp, ()> {
        self.nodes += 1;
        Ok(node)
    }
    fn fold_stmt_import(&mut self, node: ast::StmtImport) -> Result<ast::StmtImport<()>, ()> {
        for alias in &node.names {
            self.test_import |= matches!(
                alias.name.as_str().split('.').next(),
                Some("pytest" | "unittest")
            );
            self.re_import |= alias.name.as_str() == "re" && alias.asname.is_none();
        }
        fold::fold_stmt_import(self, node)
    }
    fn fold_stmt_import_from(
        &mut self,
        node: ast::StmtImportFrom,
    ) -> Result<ast::StmtImportFrom<()>, ()> {
        self.test_import |= node
            .module
            .as_ref()
            .is_some_and(|m| matches!(m.as_str().split('.').next(), Some("pytest" | "unittest")));
        fold::fold_stmt_import_from(self, node)
    }
    fn fold_expr_call(&mut self, mut node: ast::ExprCall) -> Result<ast::ExprCall<()>, ()> {
        let regex = matches!(&*node.func,ast::Expr::Attribute(a) if matches!(&*a.value,ast::Expr::Name(n) if n.id.as_str()=="re")
            && ["compile","search","match","fullmatch","findall","finditer","split","sub","subn"].contains(&a.attr.as_str()));
        if regex {
            if let Some(ast::Expr::Constant(c)) = node.args.first_mut() {
                if let ast::Constant::Str(s) = &mut c.value {
                    let index = self.patterns.len();
                    self.patterns.push(s.clone());
                    if let Some((at, value)) = &self.rewrite {
                        if *at == index {
                            *s = value.clone();
                        }
                    }
                }
            }
        }
        fold::fold_expr_call(self, node)
    }
}
// Err(true) means actual syntax failure; resource limits never prove syntax.
fn parse(source: &str) -> Result<(Tree, Normalize), bool> {
    if source.chars().count() > 8192 {
        return Err(false);
    }
    let mut depth = 0usize;
    for token in rustpython_parser::lexer::lex(source, rustpython_parser::Mode::Module) {
        let Ok((token, _)) = token else { break };
        match token {
            rustpython_parser::Tok::Lpar
            | rustpython_parser::Tok::Lsqb
            | rustpython_parser::Tok::Lbrace => {
                depth += 1;
                if depth > 200 {
                    return Err(false);
                }
            }
            rustpython_parser::Tok::Rpar
            | rustpython_parser::Tok::Rsqb
            | rustpython_parser::Tok::Rbrace => depth = depth.saturating_sub(1),
            _ => (),
        }
    }
    let source = ast::Suite::parse(source, "<string>").map_err(|_| true)?;
    let mut n = Normalize {
        nodes: 1,
        ..Default::default()
    };
    let tree = source
        .into_iter()
        .map(|s| n.fold_stmt(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| false)?;
    Ok((tree, n))
}
fn probe_source(argv: &[String]) -> Option<&str> {
    if !matches!(
        std::path::Path::new(argv.first()?).file_name()?.to_str()?,
        "python" | "python3"
    ) || argv.len() < 3
    {
        return None;
    }
    let i = argv.len() - 2;
    if argv[i] != "-c"
        || argv[1..i]
            .iter()
            .any(|s| !["-B", "-I", "-S"].contains(&s.as_str()))
    {
        return None;
    }
    Some(&argv[i + 1])
}
#[derive(Clone, Debug)]
enum Lit {
    Constant(ast::Constant),
    List(Vec<Lit>),
    Tuple(Vec<Lit>),
    Set(Vec<Lit>),
    Dict(Vec<(Lit, Lit)>),
}
fn integer(n: impl Into<BigInt>) -> Lit {
    Lit::Constant(ast::Constant::Int(n.into()))
}
fn numeric(v: &Lit) -> Option<(Option<BigInt>, f64, f64)> {
    match v {
        Lit::Constant(ast::Constant::Int(i)) => Some((Some(i.clone()), 0., 0.)),
        Lit::Constant(ast::Constant::Bool(b)) => Some((Some(BigInt::from(*b as u8)), 0., 0.)),
        Lit::Constant(ast::Constant::Float(f)) => Some((None, *f, 0.)),
        Lit::Constant(ast::Constant::Complex { real, imag }) => Some((None, *real, *imag)),
        _ => None,
    }
}
fn real_float(i: Option<BigInt>, r: f64) -> Option<f64> {
    match i {
        Some(i) => i.to_string().parse::<f64>().ok().filter(|f| f.is_finite()),
        None => Some(r),
    }
}
fn real_cmp(a: &Lit, b: &Lit) -> Option<Ordering> {
    let (ai, ar, az) = numeric(a)?;
    let (bi, br, bz) = numeric(b)?;
    if az != 0. || bz != 0. {
        return None;
    }
    match (ai, bi) {
        (Some(a), Some(b)) => Some(a.cmp(&b)),
        (None, None) => ar.partial_cmp(&br),
        (Some(a), None) => int_float_cmp(&a, br),
        (None, Some(b)) => int_float_cmp(&b, ar).map(Ordering::reverse),
    }
}
fn int_float_cmp(i: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if f == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let trunc: BigInt = format!("{:.0}", f.trunc()).parse().ok()?;
    let order = i.cmp(&trunc);
    Some(if order != Ordering::Equal {
        order
    } else if f.fract() > 0. {
        Ordering::Less
    } else if f.fract() < 0. {
        Ordering::Greater
    } else {
        Ordering::Equal
    })
}
fn equal(a: &Lit, b: &Lit) -> bool {
    if let (Some((ai, ar, az)), Some((bi, br, bz))) = (numeric(a), numeric(b)) {
        if az != bz {
            return false;
        }
        let a = ai
            .map(integer)
            .unwrap_or(Lit::Constant(ast::Constant::Float(ar)));
        let b = bi
            .map(integer)
            .unwrap_or(Lit::Constant(ast::Constant::Float(br)));
        return real_cmp(&a, &b) == Some(Ordering::Equal);
    }
    match (a, b) {
        (Lit::Constant(a), Lit::Constant(b)) => a == b,
        (Lit::List(a), Lit::List(b)) | (Lit::Tuple(a), Lit::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Lit::Set(a), Lit::Set(b)) => {
            a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| equal(x, y)))
        }
        (Lit::Dict(a), Lit::Dict(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.iter().any(|(l, w)| equal(k, l) && equal(v, w)))
        }
        _ => false,
    }
}
fn hashable(v: &Lit) -> bool {
    match v {
        Lit::Constant(_) => true,
        Lit::Tuple(v) => v.iter().all(hashable),
        _ => false,
    }
}
fn truth(v: &Lit) -> bool {
    match v {
        Lit::Constant(c) => match c {
            ast::Constant::None => false,
            ast::Constant::Bool(b) => *b,
            ast::Constant::Int(i) => i != &BigInt::from(0),
            ast::Constant::Float(f) => *f != 0.,
            ast::Constant::Complex { real, imag } => *real != 0. || *imag != 0.,
            ast::Constant::Str(s) => !s.is_empty(),
            ast::Constant::Bytes(b) => !b.is_empty(),
            ast::Constant::Tuple(a) => !a.is_empty(),
            ast::Constant::Ellipsis => true,
        },
        Lit::List(v) | Lit::Tuple(v) | Lit::Set(v) => !v.is_empty(),
        Lit::Dict(v) => !v.is_empty(),
    }
}
fn sequence(v: Lit) -> Option<Vec<Lit>> {
    match v {
        Lit::List(v) | Lit::Tuple(v) | Lit::Set(v) => Some(v),
        Lit::Dict(v) => Some(v.into_iter().map(|(k, _)| k).collect()),
        Lit::Constant(ast::Constant::Str(s)) => Some(
            s.chars()
                .map(|c| Lit::Constant(ast::Constant::Str(c.to_string())))
                .collect(),
        ),
        Lit::Constant(ast::Constant::Bytes(b)) => Some(b.into_iter().map(integer).collect()),
        _ => None,
    }
}
fn compare(a: &Lit, b: &Lit) -> Option<Ordering> {
    if matches!(a, Lit::Constant(ast::Constant::Complex { .. }))
        || matches!(b, Lit::Constant(ast::Constant::Complex { .. }))
    {
        return None;
    }
    if numeric(a).is_some() && numeric(b).is_some() {
        return real_cmp(a, b);
    }
    match (a, b) {
        (Lit::Constant(ast::Constant::Str(a)), Lit::Constant(ast::Constant::Str(b))) => {
            Some(a.cmp(b))
        }
        (Lit::Constant(ast::Constant::Bytes(a)), Lit::Constant(ast::Constant::Bytes(b))) => {
            Some(a.cmp(b))
        }
        (Lit::List(a), Lit::List(b)) | (Lit::Tuple(a), Lit::Tuple(b)) => {
            for (a, b) in a.iter().zip(b) {
                if !equal(a, b) {
                    return compare(a, b);
                }
            }
            Some(a.len().cmp(&b.len()))
        }
        _ => None,
    }
}
fn add(a: Lit, b: Lit, subtract: bool) -> Option<Lit> {
    let (ai, ar, az) = numeric(&a)?;
    let (bi, br, bz) = numeric(&b)?;
    if let (Some(a), Some(b)) = (&ai, &bi) {
        return Some(integer(if subtract { a - b } else { a + b }));
    }
    let r = real_float(ai, ar)?;
    let s = real_float(bi, br)?;
    let sign = if subtract { -1. } else { 1. };
    let real = r + sign * s;
    let imag = az + sign * bz;
    Some(
        if matches!(a, Lit::Constant(ast::Constant::Complex { .. }))
            || matches!(b, Lit::Constant(ast::Constant::Complex { .. }))
        {
            Lit::Constant(ast::Constant::Complex { real, imag })
        } else {
            Lit::Constant(ast::Constant::Float(real))
        },
    )
}
fn builtin(name: &str, arg: Lit) -> Option<Lit> {
    if name == "abs" {
        let (i, r, z) = numeric(&arg)?;
        return Some(if let Some(i) = i {
            integer(if i < BigInt::from(0) { -i } else { i })
        } else {
            Lit::Constant(ast::Constant::Float(if z == 0. {
                r.abs()
            } else {
                r.hypot(z)
            }))
        });
    }
    let values = sequence(arg)?;
    match name {
        "len" => Some(integer(values.len())),
        "sum" => values
            .into_iter()
            .try_fold(integer(0), |a, b| add(a, b, false)),
        "all" => Some(Lit::Constant(ast::Constant::Bool(values.iter().all(truth)))),
        "any" => Some(Lit::Constant(ast::Constant::Bool(values.iter().any(truth)))),
        "min" | "max" => {
            let mut it = values.into_iter();
            let mut selected = it.next()?;
            for v in it {
                let cmp = compare(&v, &selected)?;
                if (name == "min" && cmp == Ordering::Less)
                    || (name == "max" && cmp == Ordering::Greater)
                {
                    selected = v;
                }
            }
            Some(selected)
        }
        "sorted" => {
            // Bounded literal probes are small. Stable insertion sort can refuse
            // an invalid Python comparison without an infallible sort callback.
            let mut sorted: Vec<Lit> = Vec::new();
            for v in values {
                let mut at = sorted.len();
                while at > 0 && compare(&v, &sorted[at - 1])? == Ordering::Less {
                    at -= 1
                }
                sorted.insert(at, v);
            }
            Some(Lit::List(sorted))
        }
        _ => None,
    }
}
fn value(expr: &ast::Expr<()>, bindings: &BTreeMap<String, Lit>, allow: bool) -> Option<Lit> {
    match expr {
        ast::Expr::Constant(c) => Some(match &c.value {
            ast::Constant::Tuple(v) => Lit::Tuple(v.iter().cloned().map(Lit::Constant).collect()),
            c => Lit::Constant(c.clone()),
        }),
        ast::Expr::Name(n) if allow => bindings.get(n.id.as_str()).cloned(),
        ast::Expr::Call(c) if allow && c.args.len() == 1 && c.keywords.is_empty() => {
            let ast::Expr::Name(n) = &*c.func else {
                return None;
            };
            builtin(n.id.as_str(), value(&c.args[0], bindings, true)?)
        }
        ast::Expr::Call(c)
            if c.args.is_empty()
                && c.keywords.is_empty()
                && matches!(&*c.func,ast::Expr::Name(n) if n.id.as_str()=="set") =>
        {
            Some(Lit::Set(Vec::new()))
        }
        ast::Expr::UnaryOp(u) => {
            if !matches!(&*u.operand, ast::Expr::Constant(_)) {
                return None;
            }
            let v = value(&u.operand, bindings, false)?;
            match u.op {
                ast::UnaryOp::UAdd
                    if matches!(
                        v,
                        Lit::Constant(
                            ast::Constant::Int(_)
                                | ast::Constant::Float(_)
                                | ast::Constant::Complex { .. }
                        )
                    ) =>
                {
                    Some(v)
                }
                ast::UnaryOp::USub => {
                    let (i, r, z) = numeric(&v)?;
                    if matches!(v, Lit::Constant(ast::Constant::Bool(_))) {
                        return None;
                    }
                    Some(if let Some(i) = i {
                        integer(-i)
                    } else if matches!(v, Lit::Constant(ast::Constant::Complex { .. })) {
                        Lit::Constant(ast::Constant::Complex { real: -r, imag: -z })
                    } else {
                        Lit::Constant(ast::Constant::Float(-r))
                    })
                }
                _ => None,
            }
        }
        ast::Expr::BinOp(b) if matches!(b.op, ast::Operator::Add | ast::Operator::Sub) => {
            // ast.literal_eval allows only real +/- imaginary complex literals.
            let left = value(&b.left, bindings, false)?;
            let right = value(&b.right, bindings, false)?;
            if !matches!(
                left,
                Lit::Constant(ast::Constant::Int(_) | ast::Constant::Float(_))
            ) || !matches!(right, Lit::Constant(ast::Constant::Complex { .. }))
            {
                return None;
            }
            add(left, right, b.op == ast::Operator::Sub)
        }
        ast::Expr::List(l) => l
            .elts
            .iter()
            .map(|e| value(e, bindings, false))
            .collect::<Option<Vec<_>>>()
            .map(Lit::List),
        ast::Expr::Tuple(l) => l
            .elts
            .iter()
            .map(|e| value(e, bindings, false))
            .collect::<Option<Vec<_>>>()
            .map(Lit::Tuple),
        ast::Expr::Set(l) => {
            let mut values = Vec::new();
            for e in &l.elts {
                let v = value(e, bindings, false)?;
                if !hashable(&v) {
                    return None;
                }
                if !values.iter().any(|old| equal(old, &v)) {
                    values.push(v);
                }
            }
            Some(Lit::Set(values))
        }
        ast::Expr::Dict(d) => {
            let mut values: Vec<(Lit, Lit)> = Vec::new();
            for (k, v) in d.keys.iter().zip(&d.values) {
                let k = value(k.as_ref()?, bindings, false)?;
                let v = value(v, bindings, false)?;
                if !hashable(&k) {
                    return None;
                }
                if let Some((_, old)) = values.iter_mut().find(|(old, _)| equal(old, &k)) {
                    *old = v;
                } else {
                    values.push((k, v));
                }
            }
            Some(Lit::Dict(values))
        }
        _ => None,
    }
}
const BUILTINS: [&str; 8] = ["len", "sorted", "sum", "min", "max", "all", "any", "abs"];
fn expectations(tree: &Tree, nodes: usize) -> Option<Vec<(usize, bool)>> {
    if nodes > 256 {
        return None;
    }
    let mut bindings = BTreeMap::new();
    let mut assertions = Vec::new();
    for (index, node) in tree.iter().enumerate() {
        match node {
            ast::Stmt::Assign(a) if a.targets.len() == 1 => {
                let ast::Expr::Name(n) = &a.targets[0] else {
                    return None;
                };
                if bindings.contains_key(n.id.as_str()) || BUILTINS.contains(&n.id.as_str()) {
                    return None;
                }
                let v = value(&a.value, &bindings, true)?;
                bindings.insert(n.id.to_string(), v);
            }
            ast::Stmt::Assert(a) => {
                let ast::Expr::Compare(c) = &*a.test else {
                    return None;
                };
                if c.ops != [ast::CmpOp::Eq] || c.comparators.len() != 1 {
                    return None;
                }
                let expected = value(&c.comparators[0], &bindings, false)?;
                let actual = value(&c.left, &bindings, true)?;
                if let Some(msg) = &a.msg {
                    value(msg, &bindings, true)?;
                }
                assertions.push((index, equal(&actual, &expected)));
            }
            _ => return None,
        }
    }
    if assertions.is_empty() {
        None
    } else {
        Some(assertions)
    }
}
fn expected(tree: &Tree, index: usize) -> Option<&ast::Expr<()>> {
    let ast::Stmt::Assert(a) = &tree[index] else {
        return None;
    };
    let ast::Expr::Compare(c) = &*a.test else {
        return None;
    };
    c.comparators.first()
}
fn expectation_correction(before: &Tree, bn: usize, after: &Tree, an: usize) -> bool {
    let (Some(b), Some(a)) = (expectations(before, bn), expectations(after, an)) else {
        return false;
    };
    if b.len() != a.len()
        || b.iter().filter(|(_, holds)| !*holds).count() != 1
        || a.iter().any(|(_, holds)| !*holds)
    {
        return false;
    }
    let changed: Vec<_> = b
        .iter()
        .zip(&a)
        .filter(|((i, _), (j, _))| expected(before, *i) != expected(after, *j))
        .collect();
    if changed.len() != 1 {
        return false;
    }
    let ((i, holds), (j, _)) = changed[0];
    if *holds {
        return false;
    }
    let mut replaced = before.clone();
    let ast::Stmt::Assert(s) = &mut replaced[*i] else {
        return false;
    };
    let ast::Expr::Compare(c) = &mut *s.test else {
        return false;
    };
    c.comparators[0] = expected(after, *j).unwrap().clone();
    replaced == *after
}
fn script_correction(bad: &[String], good: &[String], output: &str, exit: i64) -> bool {
    if bad == good
        || exit != 2
        || bad.is_empty()
        || good.is_empty()
        || !matches!(
            std::path::Path::new(&bad[0])
                .file_name()
                .and_then(|s| s.to_str()),
            Some("python" | "python3")
        )
    {
        return false;
    }
    let mut i = 1;
    while i < bad.len() && ["-B", "-I", "-S"].contains(&bad[i].as_str()) {
        i += 1
    }
    if i >= bad.len() || !bad[i].ends_with(".py") || good.get(..=i) != bad.get(..=i) {
        return false;
    }
    let Some(name) = std::path::Path::new(&bad[i])
        .file_name()
        .and_then(|s| s.to_str())
    else {
        return false;
    };
    let lines: Vec<_> = output.trim_end_matches('\n').lines().collect();
    lines.len() >= 2
        && lines[0].starts_with(&format!("usage: {name}"))
        && lines[1..lines.len() - 1]
            .iter()
            .all(|s| s.starts_with(' ') || s.starts_with('\t'))
        && [
            "unrecognized arguments: ",
            "the following arguments are required: ",
        ]
        .iter()
        .any(|kind| {
            lines
                .last()
                .unwrap()
                .strip_prefix(&format!("{name}: error: {kind}"))
                .is_some_and(|s| !s.is_empty())
        })
}
pub(super) fn eligible(
    bad: &[String],
    good: &[String],
    output: &str,
    exit: i64,
    command: bool,
) -> bool {
    if command && script_correction(bad, good, output, exit) {
        return true;
    }
    let (Some(before), Some(after)) = (probe_source(bad), probe_source(good)) else {
        return false;
    };
    if bad[..bad.len() - 1] != good[..good.len() - 1]
        || before == after
        || before.chars().count() > 8192
        || after.chars().count() > 8192
    {
        return false;
    }
    let Ok((corrected, cn)) = parse(after) else {
        return false;
    };
    let last = output.trim_end().lines().last().unwrap_or("");
    let (original, on) = match parse(before) {
        Ok(parsed) => parsed,
        Err(syntax) => return syntax && last.starts_with("SyntaxError: "),
    };
    if on.test_import || !output.contains("File \"<string>\"") {
        return false;
    }
    if (last == "AssertionError" || last.starts_with("AssertionError:"))
        && expectation_correction(&original, on.nodes, &corrected, cn.nodes)
    {
        return true;
    }
    if !on.re_import
        || !(last.starts_with("re.error:")
            || last.starts_with("re.PatternError:")
            || last.starts_with("AssertionError:"))
        || on.patterns.len() != cn.patterns.len()
    {
        return false;
    }
    let changed: Vec<_> = on
        .patterns
        .iter()
        .zip(&cn.patterns)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .collect();
    if changed.len() != 1 {
        return false;
    }
    let (at, (_, new)) = changed[0];
    let Ok(tree) = ast::Suite::parse(before, "<string>") else {
        return false;
    };
    let mut n = Normalize {
        rewrite: Some((at, new.clone())),
        ..Default::default()
    };
    tree.into_iter()
        .map(|s| n.fold_stmt(s))
        .collect::<Result<Vec<_>, _>>()
        .ok()
        == Some(corrected)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn probe(s: &str) -> Vec<String> {
        vec!["python3".into(), "-B".into(), "-c".into(), s.into()]
    }
    #[test]
    fn literal_and_source_safety_tables() {
        let output = "File \"<string>\"\nAssertionError\n";
        for (bad, good) in [
            (
                "items=[1,2]; assert len(items)==3",
                "items=[1,2,3]; assert len(items)==3",
            ),
            (
                "items=[1,2]; assert len(items)==3",
                "items=[1,2]; assert True",
            ),
            ("assert 1==2; assert 2==3", "assert 1==1; assert 2==2"),
            (
                "items=[1,2]; assert len(items)==3; assert sum(items)==3",
                "items=[1,2]; assert len(items)==2",
            ),
            (
                "import pytest; assert len([1,2])==3",
                "import pytest; assert len([1,2])==2",
            ),
            (
                "import subprocess; subprocess.run(['true']); assert len([1,2])==3",
                "import subprocess; subprocess.run(['true']); assert len([1,2])==2",
            ),
            ("exec('assert False')", "exec('assert True')"),
            ("x=1; x=2; assert x==3", "x=1; x=2; assert x==2"),
            ("len=1; assert len==2", "len=1; assert len==1"),
            ("assert len(1)==2", "assert len(1)==1"),
            ("assert sum([1,'x'])==2", "assert sum([1,'x'])==1"),
        ] {
            assert!(
                !eligible(&probe(bad), &probe(good), output, 1, false),
                "{bad}"
            );
        }
        for (expression, bad, good) in [
            ("len(['a','b'])", "3", "2"),
            ("sorted([2,1])", "[2,1]", "[1,2]"),
            ("sum([1,2])", "4", "3"),
            ("min([2,1])", "2", "1"),
            ("max([2,1])", "1", "2"),
            ("all([True,False])", "True", "False"),
            ("any([True,False])", "False", "True"),
            ("abs(-2)", "-2", "2"),
            ("len((1,2))", "3", "2"),
            ("len({'a':1,'b':2})", "3", "2"),
            ("len({1,1,2})", "3", "2"),
            ("len(b'abc')", "4", "3"),
        ] {
            assert!(
                eligible(
                    &probe(&format!("assert {expression}=={bad}")),
                    &probe(&format!("assert {expression}=={good}")),
                    output,
                    1,
                    false
                ),
                "{expression}"
            );
        }
        assert!(eligible(
            &probe("assert 9007199254740993==9007199254740992.0"),
            &probe("assert 9007199254740993==9007199254740993"),
            output,
            1,
            false
        ));
        for bad in ["a =", "if True print(1)", "return = 1"] {
            assert!(
                eligible(
                    &probe(bad),
                    &probe("assert True"),
                    "SyntaxError: invalid syntax",
                    1,
                    false
                ),
                "{bad}"
            );
        }
        assert!(eligible(
            &probe("import re\nre.search(r'(', 'subject')"),
            &probe("import re; re.search('subject', 'subject')"),
            "File \"<string>\"\nre.error: bad pattern",
            1,
            false
        ));
        assert!(!eligible(
            &probe("assert True==1"),
            &probe("assert True==True"),
            output,
            1,
            false
        ));
        let bad = probe("items=[1,2]; assert len(items)==3");
        let good = probe("items=[1,2]; assert len(items)==2");
        for diagnostic in ["AssertionError\n", "File \"<string>\"\nValueError\n"] {
            assert!(!eligible(&bad, &good, diagnostic, 1, false));
        }
        let huge = format!("assert len([1,2])==3{}", " ".repeat(8192));
        assert!(!eligible(&probe(&huge), &good, output, 1, false));
        assert!(!eligible(
            &probe("print('\\x41'); print('SyntaxError: fake')"),
            &probe("assert True"),
            "SyntaxError: fake",
            1,
            false
        ));
    }
}
