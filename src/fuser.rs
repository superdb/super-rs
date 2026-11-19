//! The fuser, which computes the fused type of a sequence of types.  This is
//! a port of the Fuser of SuperDB (fuser.go and fieldorder.go), which
//! BSUP writers use to compute the fused types of frames and super frames,
//! so this crate writes the same fused types as SuperDB.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::types::{flatten, Field, Kind, Primitive, Type};

/// Fuser computes a fused supertype of all the types passed to
/// [`Fuser::fuse`].
///
/// The supertype is a type of the type system: it has no fusion types and
/// `any` absorbs every type except `missing`.  The fused type (see
/// [`Fuser::fused_type`]) is a type of the data model, which marks with
/// fusion types where the supertype differs from the fused types so that
/// each value can carry its own type.
#[derive(Default)]
pub struct Fuser {
    sup: Option<Type>,
    subtypes: Vec<Type>,
    seen: HashSet<Type>,
    ordered: Option<Type>,
    fused: HashMap<(Type, Type), Type>,
    merged: HashMap<(Type, Type), Type>,
    normalized: HashMap<Type, Type>,
}

impl Fuser {
    /// Returns a new fuser.
    pub fn new() -> Fuser {
        Fuser::default()
    }

    /// Returns true if no types have been fused.
    pub fn is_empty(&self) -> bool {
        self.sup.is_none()
    }

    /// Fuses `t` into the supertype.
    pub fn fuse(&mut self, t: &Type) {
        if !self.seen.insert(t.clone()) {
            return;
        }
        self.subtypes.push(t.clone());
        self.ordered = None;
        let t = self.normalize(t);
        self.sup = Some(match self.sup.take() {
            None => t,
            Some(s) => self.fuse2(&s, &t),
        });
    }

    /// Returns the supertype with the fields of each record type ordered by
    /// the field orders of the fused types, or `None` if no types have been
    /// fused.
    pub fn super_type(&mut self) -> Option<Type> {
        if self.ordered.is_none() {
            if let Some(sup) = self.sup.clone() {
                let mut o = FieldOrderer::default();
                for sub in &self.subtypes {
                    o.vote(&sup, sub);
                }
                self.ordered = Some(o.reorder(&sup));
            }
        }
        self.ordered.clone()
    }

    /// Returns the fused type of the types fused so far, or `None` if no
    /// types have been fused.
    pub fn fused_type(&mut self) -> Option<Type> {
        let mut fusion = self.super_type()?;
        let subtypes = self.subtypes.clone();
        for sub in &subtypes {
            fusion = self.merge(&fusion, sub);
        }
        Some(any_to_fusion(&fusion))
    }

    fn fuse2(&mut self, a: &Type, b: &Type) -> Type {
        if a == b {
            return a.clone();
        }
        let key = (a.clone(), b.clone());
        if let Some(t) = self.fused.get(&key) {
            return t.clone();
        }
        let t = self.fuse1(a, b);
        self.fused.insert(key, t.clone());
        t
    }

    fn fuse1(&mut self, a: &Type, b: &Type) -> Type {
        if let Type::Fusion(t) = a {
            return self.fuse2(t, b);
        }
        if let Type::Fusion(t) = b {
            return self.fuse2(a, t);
        }
        if is_any(a) || is_any(b) {
            return any_type(has_missing(a) || has_missing(b));
        }
        match (a, b) {
            (Type::Record(af), Type::Record(bf)) => {
                let mut fields = Vec::new();
                for f in af.iter() {
                    let t = match bf.iter().find(|g| g.name == f.name) {
                        Some(g) => self.fuse2(&f.ty, &g.ty),
                        None => self.fuse2(&f.ty, &Type::MISSING),
                    };
                    fields.push(Field::new(f.name.clone(), t));
                }
                for g in bf.iter() {
                    if af.iter().any(|f| f.name == g.name) {
                        continue;
                    }
                    let t = self.fuse2(&g.ty, &Type::MISSING);
                    fields.push(Field::new(g.name.clone(), t));
                }
                fields.sort_by(|x, y| x.name.cmp(&y.name));
                return Type::Record(fields.into());
            }
            (Type::Array(x), Type::Array(y)) => return Type::array(self.fuse_elem(x, y)),
            (Type::Set(x), Type::Set(y)) => return Type::set(self.fuse_elem(x, y)),
            (Type::Map(x), Type::Map(y)) => {
                let k = self.fuse_elem(&x.0, &y.0);
                let v = self.fuse_elem(&x.1, &y.1);
                return Type::map(k, v);
            }
            (Type::Union(_), _) => {
                let mut types = Vec::new();
                self.extend_union(&mut types, a, Combine::Fuse);
                self.extend_union(&mut types, b, Combine::Fuse);
                return union_of(types);
            }
            (Type::Enum(x), Type::Enum(y)) => {
                let added: Vec<String> = y.iter().filter(|s| !x.contains(s)).cloned().collect();
                if added.is_empty() {
                    return a.clone();
                }
                let mut symbols = x.to_vec();
                symbols.extend(added);
                return Type::enumeration(symbols);
            }
            (Type::Error(x), Type::Error(y)) => return Type::error(self.fuse2(x, y)),
            (Type::Option(x), Type::Option(y)) => return option(self.fuse2(x, y)),
            _ => {}
        }
        // Named types are a barrier to fusion: they're fused with the other
        // type into a union.
        if matches!(b, Type::Union(_) | Type::Option(_)) {
            return self.fuse2(b, a);
        }
        make_union(vec![a.clone(), b.clone()])
    }

    /// Fuses the element types of two containers.  The element type of an
    /// empty container is `none`, which is absorbed by the other.
    fn fuse_elem(&mut self, a: &Type, b: &Type) -> Type {
        if *a == Type::NONE {
            return b.clone();
        }
        if *b == Type::NONE {
            return a.clone();
        }
        self.fuse2(a, b)
    }

    fn normalize(&mut self, t: &Type) -> Type {
        if let Some(out) = self.normalized.get(t) {
            return out.clone();
        }
        let out = self.normalize1(t);
        self.normalized.insert(t.clone(), out.clone());
        out
    }

    fn normalize1(&mut self, t: &Type) -> Type {
        if let Type::Fusion(inner) = t {
            return self.normalize(inner);
        }
        if is_any(t) {
            return Type::ANY;
        }
        match t {
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|f| Field::new(f.name.clone(), self.normalize(&f.ty)))
                    .collect(),
            ),
            Type::Array(x) => Type::array(self.normalize(x)),
            Type::Set(x) => Type::set(self.normalize(x)),
            Type::Map(kv) => {
                let k = self.normalize(&kv.0);
                Type::map(k, self.normalize(&kv.1))
            }
            Type::Union(members) => {
                let mut types = Vec::new();
                for m in members.iter() {
                    let m = self.normalize(m);
                    self.extend_union(&mut types, &m, Combine::Fuse);
                }
                union_of(types)
            }
            Type::Option(x) => option(self.normalize(x)),
            Type::Error(x) => Type::error(self.normalize(x)),
            _ => t.clone(),
        }
    }

    fn merge(&mut self, sup: &Type, sub: &Type) -> Type {
        let key = (sup.clone(), sub.clone());
        if let Some(t) = self.merged.get(&key) {
            return t.clone();
        }
        let t = self.merge1(sup, sub);
        self.merged.insert(key, t.clone());
        t
    }

    fn merge1(&mut self, sup: &Type, sub: &Type) -> Type {
        if is_any(sup) || union_has_any(sup) {
            // A value fused into any is a value of fusion(all), which
            // carries the value's own type.
            return fusion_all();
        }
        if sup == sub {
            return sup.clone();
        }
        if let Type::Fusion(inner) = sup {
            return fusion(self.merge(inner, sub));
        }
        if let Type::Fusion(inner) = sub {
            return fusion(self.merge(sup, inner));
        }
        if let Type::Union(members) = sub {
            if !matches!(sup, Type::Union(_)) {
                // The members of sub were fused into the single type sup.
                let mut s = sup.clone();
                for m in members.iter() {
                    s = no_fusion(&self.merge(&s, m));
                }
                return fusion(s);
            }
        }
        match (sup, sub) {
            (Type::Record(sf), Type::Record(bf)) => {
                let mut fields: Vec<Field> = sf.to_vec();
                for g in bf.iter() {
                    if let Some(k) = sf.iter().position(|f| f.name == g.name) {
                        fields[k].ty = self.merge(&fields[k].ty, &g.ty);
                    }
                }
                // A field of sup that's not in sub is missing from sub's
                // values.
                for (k, f) in sf.iter().enumerate() {
                    if !bf.iter().any(|g| g.name == f.name) {
                        fields[k].ty = self.merge(&fields[k].ty, &Type::MISSING);
                    }
                }
                let t = Type::Record(fields.into());
                if !order_preserved(sf, bf) {
                    return fusion(t);
                }
                t
            }
            (Type::Array(x), Type::Array(y)) => fusion(Type::array(self.merge_elem(x, y))),
            (Type::Set(x), Type::Set(y)) => fusion(Type::set(self.merge_elem(x, y))),
            (Type::Map(x), Type::Map(y)) => {
                let k = self.merge_elem(&x.0, &y.0);
                let v = self.merge_elem(&x.1, &y.1);
                fusion(Type::map(k, v))
            }
            (Type::Union(members), _) => {
                let mut types = members.to_vec();
                self.extend_union(&mut types, sub, Combine::Merge);
                if types.len() == 1 {
                    return types.pop().unwrap();
                }
                fusion(make_union(types))
            }
            (Type::Enum(x), Type::Enum(y)) => {
                let added: Vec<String> = y.iter().filter(|s| !x.contains(s)).cloned().collect();
                if added.is_empty() {
                    return sup.clone();
                }
                let mut symbols = x.to_vec();
                symbols.extend(added);
                fusion(Type::enumeration(symbols))
            }
            (Type::Error(x), Type::Error(y)) => fusion(Type::error(self.merge(x, y))),
            (Type::Option(x), Type::Option(y)) => {
                let t = no_fusion(&self.merge(x, y));
                fusion(option(t))
            }
            // A named type is a barrier to fusion, as is any other pairing
            // the supertype doesn't allow.
            _ => sup.clone(),
        }
    }

    fn merge_elem(&mut self, a: &Type, b: &Type) -> Type {
        if *a == Type::NONE {
            return b.clone();
        }
        if *b == Type::NONE {
            return a.clone();
        }
        self.merge(a, b)
    }

    /// Adds `t` to `types` maintaining the invariant that `types` holds at
    /// most one unnamed type of each complex kind and no unions.  A type of
    /// the same complex kind as a member is combined with it.
    fn extend_union(&mut self, types: &mut Vec<Type>, t: &Type, combine: Combine) {
        match t {
            Type::Named(n) => {
                if !types
                    .iter()
                    .any(|m| matches!(m, Type::Named(x) if x.name() == n.name()))
                {
                    types.push(t.clone());
                }
                return;
            }
            Type::Union(members) => {
                for m in members.iter() {
                    self.extend_union(types, m, combine);
                }
                return;
            }
            Type::Fusion(inner) => return self.extend_union(types, inner, combine),
            _ => {}
        }
        let kind = t.kind();
        #[allow(clippy::needless_range_loop)]
        for i in 0..types.len() {
            if types[i] == *t {
                return;
            }
            if kind != Kind::Primitive && kind == types[i].kind() && !types[i].is_named() {
                let m = types[i].clone();
                let c = match combine {
                    Combine::Fuse => self.fuse2(&m, t),
                    Combine::Merge => self.merge(&m, t),
                };
                types[i] = no_fusion(&c);
                return;
            }
        }
        types.push(no_fusion(t));
    }
}

#[derive(Clone, Copy)]
enum Combine {
    Fuse,
    Merge,
}

fn is_any(t: &Type) -> bool {
    matches!(t, Type::Primitive(Primitive::Any | Primitive::All))
}

fn has_missing(t: &Type) -> bool {
    match t {
        Type::Primitive(Primitive::Missing) => true,
        Type::Union(members) => members.contains(&Type::MISSING),
        _ => false,
    }
}

fn any_type(missing: bool) -> Type {
    if missing {
        make_union(vec![Type::ANY, Type::MISSING])
    } else {
        Type::ANY
    }
}

fn union_of(types: Vec<Type>) -> Type {
    if types.iter().any(is_any) {
        return any_type(types.contains(&Type::MISSING));
    }
    if types.len() == 1 {
        return types.into_iter().next().unwrap();
    }
    make_union(types)
}

fn make_union(types: Vec<Type>) -> Type {
    let types = flatten(types);
    if types.len() == 1 {
        return types.into_iter().next().unwrap();
    }
    Type::Union(types.into())
}

fn union_has_any(t: &Type) -> bool {
    matches!(t, Type::Union(members) if members.iter().any(is_any))
}

fn no_fusion(t: &Type) -> Type {
    match t {
        Type::Fusion(inner) => (**inner).clone(),
        _ => t.clone(),
    }
}

fn fusion(t: Type) -> Type {
    match t {
        Type::Fusion(_) => t,
        _ => Type::fusion(t),
    }
}

fn fusion_all() -> Type {
    Type::fusion(Type::ALL)
}

fn option(t: Type) -> Type {
    Type::Option(Arc::new(t))
}

fn order_preserved(fused: &[Field], child: &[Field]) -> bool {
    let index = |name: &str| {
        fused
            .iter()
            .position(|g| g.name == name)
            .map_or(-1, |i| i as i64)
    };
    let mut off: i64 = -1;
    for f in child {
        if off < 0 {
            off = index(&f.name);
            continue;
        }
        let next = index(&f.name);
        if next < off {
            return false;
        }
        off = next;
    }
    true
}

fn has_any(t: &Type) -> bool {
    match t {
        Type::Primitive(Primitive::Any) => true,
        Type::Record(fields) => fields.iter().any(|f| has_any(&f.ty)),
        Type::Array(x) | Type::Set(x) | Type::Error(x) | Type::Option(x) | Type::Fusion(x) => {
            has_any(x)
        }
        Type::Map(kv) => has_any(&kv.0) || has_any(&kv.1),
        Type::Union(members) => members.iter().any(has_any),
        _ => false,
    }
}

/// Replaces each occurrence of `any`, which exists only in the type system,
/// with `fusion(all)`.
pub(crate) fn any_to_fusion(t: &Type) -> Type {
    if !has_any(t) {
        return t.clone();
    }
    match t {
        Type::Primitive(Primitive::Any) => fusion_all(),
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|f| Field::new(f.name.clone(), any_to_fusion(&f.ty)))
                .collect(),
        ),
        Type::Array(x) => Type::array(any_to_fusion(x)),
        Type::Set(x) => Type::set(any_to_fusion(x)),
        Type::Map(kv) => Type::map(any_to_fusion(&kv.0), any_to_fusion(&kv.1)),
        Type::Union(members) => {
            if members.contains(&Type::ANY) {
                return fusion_all();
            }
            make_union(members.iter().map(any_to_fusion).collect())
        }
        Type::Error(x) => Type::error(any_to_fusion(x)),
        Type::Option(x) => option(any_to_fusion(x)),
        Type::Fusion(x) => {
            let inner = any_to_fusion(x);
            if inner == fusion_all() {
                inner
            } else {
                fusion(inner)
            }
        }
        _ => t.clone(),
    }
}

/// Orders the fields of the record types of a supertype by rank aggregation
/// over the field orders of the fused types (see fieldorder.go in SuperDB).
#[derive(Default)]
struct FieldOrderer {
    votes: HashMap<Type, Vec<Vec<i32>>>,
    memo: HashMap<Type, Type>,
}

impl FieldOrderer {
    fn vote(&mut self, sup: &Type, sub: &Type) {
        let sub = no_fusion(sub);
        if let Type::Union(members) = &sub {
            if !matches!(sup, Type::Union(_)) {
                for m in members.iter() {
                    self.vote(sup, m);
                }
                return;
            }
        }
        match sup {
            Type::Record(sf) => {
                let Type::Record(bf) = &sub else { return };
                self.vote_record(sup, sf, bf);
                for g in bf.iter() {
                    if let Some(f) = sf.iter().find(|f| f.name == g.name) {
                        self.vote(&f.ty, &g.ty);
                    }
                }
            }
            Type::Array(x) => {
                if let Type::Array(y) = &sub {
                    self.vote(x, y);
                }
            }
            Type::Set(x) => {
                if let Type::Set(y) = &sub {
                    self.vote(x, y);
                }
            }
            Type::Map(x) => {
                if let Type::Map(y) = &sub {
                    self.vote(&x.0, &y.0);
                    self.vote(&x.1, &y.1);
                }
            }
            Type::Error(x) => {
                if let Type::Error(y) = &sub {
                    self.vote(x, y);
                }
            }
            Type::Option(x) => {
                if let Type::Option(y) = &sub {
                    self.vote(x, y);
                }
            }
            Type::Union(members) => {
                if let Type::Union(sm) = &sub {
                    for m in sm.iter() {
                        self.vote(sup, m);
                    }
                    return;
                }
                if sub.is_named() || sub.kind() == Kind::Primitive {
                    return;
                }
                for m in members.iter() {
                    if m.kind() == sub.kind() && !m.is_named() {
                        self.vote(m, &sub);
                        return;
                    }
                }
            }
            // Named types are barriers to fusion so their bodies aren't
            // reordered.
            _ => {}
        }
    }

    fn vote_record(&mut self, sup: &Type, sf: &[Field], bf: &[Field]) {
        let n = sf.len();
        let margins = self
            .votes
            .entry(sup.clone())
            .or_insert_with(|| vec![vec![0; n]; n]);
        let cols: Vec<usize> = bf
            .iter()
            .filter_map(|g| sf.iter().position(|f| f.name == g.name))
            .collect();
        for (i, &x) in cols.iter().enumerate() {
            for &y in &cols[i + 1..] {
                margins[x][y] += 1;
                margins[y][x] -= 1;
            }
        }
    }

    fn reorder(&mut self, t: &Type) -> Type {
        if let Some(out) = self.memo.get(t) {
            return out.clone();
        }
        let out = match t {
            Type::Record(fields) => {
                let mut fields: Vec<Field> = fields
                    .iter()
                    .map(|f| Field::new(f.name.clone(), self.reorder(&f.ty)))
                    .collect();
                if let Some(margins) = self.votes.get(t) {
                    fields = rank(margins, fields);
                }
                Type::Record(fields.into())
            }
            Type::Array(x) => Type::array(self.reorder(x)),
            Type::Set(x) => Type::set(self.reorder(x)),
            Type::Map(kv) => {
                let k = self.reorder(&kv.0);
                Type::map(k, self.reorder(&kv.1))
            }
            Type::Error(x) => Type::error(self.reorder(x)),
            Type::Option(x) => option(self.reorder(x)),
            Type::Union(members) => {
                let members: Vec<Type> = members.iter().map(|m| self.reorder(m)).collect();
                make_union(members)
            }
            _ => t.clone(),
        };
        self.memo.insert(t.clone(), out.clone());
        out
    }
}

/// Orders `fields` by the pairwise margins: it repeatedly takes the best
/// remaining field (by Copeland score, then net margin, then name) that no
/// remaining field beats.
fn rank(margins: &[Vec<i32>], fields: Vec<Field>) -> Vec<Field> {
    let n = fields.len();
    let mut copeland = vec![0i64; n];
    let mut net = vec![0i64; n];
    let mut beaten = vec![0i64; n];
    for i in 0..n {
        for (j, &m) in margins[i].iter().enumerate() {
            if j == i {
                continue;
            }
            if m > 0 {
                copeland[i] += 1;
            } else if m < 0 {
                copeland[i] -= 1;
                beaten[i] += 1;
            }
            net[i] += i64::from(m);
        }
    }
    let mut taken = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut best: Option<usize> = None;
        let mut best_unbeaten = false;
        for i in 0..n {
            if taken[i] {
                continue;
            }
            let unbeaten = beaten[i] == 0;
            let better = match best {
                None => true,
                Some(b) => {
                    (unbeaten && !best_unbeaten)
                        || (unbeaten == best_unbeaten
                            && (copeland[i] > copeland[b]
                                || (copeland[i] == copeland[b]
                                    && (net[i] > net[b]
                                        || (net[i] == net[b] && fields[i].name < fields[b].name)))))
                }
            };
            if better {
                best = Some(i);
                best_unbeaten = unbeaten;
            }
        }
        let b = best.unwrap();
        taken[b] = true;
        out.push(fields[b].clone());
        for (j, &m) in margins[b].iter().enumerate() {
            if taken[j] {
                continue;
            }
            if m > 0 {
                beaten[j] -= 1;
                copeland[j] += 1;
            } else if m < 0 {
                copeland[j] -= 1;
            }
            net[j] += i64::from(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(fields: &[(&str, Type)]) -> Type {
        Type::record(
            fields
                .iter()
                .map(|(n, t)| Field::new(*n, t.clone()))
                .collect(),
        )
        .unwrap()
    }

    fn fused(types: &[Type]) -> String {
        let mut f = Fuser::new();
        for t in types {
            f.fuse(t);
        }
        f.fused_type().unwrap().to_string()
    }

    #[test]
    fn same_type() {
        let t = rec(&[("b", Type::INT64), ("a", Type::STRING)]);
        assert_eq!(fused(&[t.clone(), t]), "{b:int64,a:string}");
    }

    #[test]
    fn records() {
        assert_eq!(
            fused(&[
                rec(&[("a", Type::INT64)]),
                rec(&[("a", Type::INT64), ("b", Type::STRING)])
            ]),
            "{a:int64,b:fusion(string|missing)}"
        );
        assert_eq!(
            fused(&[rec(&[("a", Type::INT64)]), rec(&[("a", Type::STRING)])]),
            "{a:fusion(int64|string)}"
        );
        assert_eq!(
            fused(&[
                rec(&[("a", Type::INT64), ("b", Type::INT64)]),
                rec(&[("b", Type::INT64), ("a", Type::INT64)])
            ]),
            "fusion({a:int64,b:int64})"
        );
    }

    #[test]
    fn primitives_and_containers() {
        assert_eq!(fused(&[Type::INT64, Type::STRING]), "fusion(int64|string)");
        assert_eq!(
            fused(&[Type::array(Type::NONE), Type::array(Type::INT64)]),
            "fusion([int64])"
        );
        assert_eq!(fused(&[Type::INT64]), "int64");
    }
}
