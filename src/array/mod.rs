// Copyright 2024 RisingLight Project Authors. Licensed under Apache-2.0.

use std::convert::TryFrom;
use std::fmt::Debug;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

use bitvec::vec::BitVec;
use paste::paste;
use rust_decimal::prelude::FromStr;
use rust_decimal::Decimal;

use crate::types::{
    Blob, ConvertError, DataType, DataValue, Date, Interval, Timestamp, TimestampTz, Vector, F32,
    F64,
};

mod chunked_array;
mod data_chunk;
mod data_chunk_builder;
pub mod ops;
mod primitive_array;
mod var_array;

pub use self::chunked_array::*;
pub use self::data_chunk::*;
pub use self::data_chunk_builder::*;
pub use self::primitive_array::*;
pub use self::var_array::*;

mod internal_ext;

pub use internal_ext::*;

mod shuffle_ext;

pub use shuffle_ext::*;

/// A trait over all array builders.
///
/// `ArrayBuilder` is a trait over all builders. You could build an array with
/// `push` with the help of `ArrayBuilder` trait. The `push` function always
/// accepts reference to an element. e.g. for `PrimitiveArray`,
/// you must do `builder.push(Some(&1))`. For `StringArray`, you must do
/// `builder.push(Some("xxx"))`. Note that you don't need to construct a `String`.
///
/// The associated type `Array` is the type of the corresponding array. It is the
/// return type of `finish`.
pub trait ArrayBuilder: Sized + Send + Sync + 'static {
    /// Corresponding `Array` of this builder
    type Array: Array<Builder = Self>;

    /// Create a new builder.
    fn new() -> Self {
        Self::with_capacity(0)
    }

    fn extend_from_raw_data(&mut self, raw: &[<<Self::Array as Array>::Item as ToOwned>::Owned]);

    fn extend_from_nulls(&mut self, count: usize);

    fn replace_bitmap(&mut self, valid: BitVec);

    /// Create a new builder with `capacity`.
    fn with_capacity(capacity: usize) -> Self;

    /// Reserve at least `capacity` values.
    fn reserve(&mut self, capacity: usize);

    /// Append a value to builder.
    fn push(&mut self, value: Option<&<Self::Array as Array>::Item>) {
        self.push_n(1, value);
    }

    /// Append a value multiple times.
    fn push_n(&mut self, n: usize, value: Option<&<Self::Array as Array>::Item>);

    /// Append an array to builder.
    fn append(&mut self, other: &Self::Array);

    /// Take all elements and return a new array.
    fn take(&mut self) -> Self::Array;

    /// Finish build and return a new array.
    fn finish(mut self) -> Self::Array {
        self.take()
    }
}

/// A trait over all array.
///
/// `Array` must be built with an `ArrayBuilder`. The array trait provides several
/// unified interface on an array, like `len`, `get` and `iter`.
///
/// The `Builder` associated type is the builder for this array.
/// The `Item` is the item you could retrieve from this array.
///
/// For example, `PrimitiveArray` could return an `Option<&u32>`, and `StringArray` will
/// return an `Option<&str>`.
pub trait Array: Sized + Send + Sync + 'static {
    /// Corresponding builder of this array.
    type Builder: ArrayBuilder<Array = Self>;
    /// Type of element in the array.
    type Item: ToOwned + ?Sized;

    /// Returns true if the value at `idx` is null.
    fn is_null(&self, idx: usize) -> bool;

    /// Returns the raw value at `idx` regardless of null.
    fn get_raw(&self, idx: usize) -> &Self::Item;

    /// Number of items of array.
    fn len(&self) -> usize;

    /// Retrieve a reference to value.
    fn get(&self, idx: usize) -> Option<&Self::Item> {
        if self.is_null(idx) {
            None
        } else {
            Some(self.get_raw(idx))
        }
    }

    fn filter(&self, p: &[bool]) -> Self;

    /// Get iterator of current array.
    fn iter(&self) -> impl DoubleEndedIterator<Item = Option<&Self::Item>> {
        (0..self.len()).map(|i| self.get(i))
    }

    /// Get iterator over the raw values.
    fn raw_iter(&self) -> impl DoubleEndedIterator<Item = &Self::Item> {
        (0..self.len()).map(|i| self.get_raw(i))
    }

    /// Get iterator over the non-null values.
    fn nonnull_iter(&self) -> impl DoubleEndedIterator<Item = &Self::Item> {
        (0..self.len())
            .filter(|i| !self.is_null(*i))
            .map(|i| self.get_raw(i))
    }

    /// Check if `Array` is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An extension trait for [`Array`].
pub trait ArrayExt: Array {
    /// Return a slice of self for the provided range.
    fn slice(&self, range: impl RangeBounds<usize>) -> Self;
}

impl<A: Array> ArrayExt for A {
    /// Return a slice of self for the provided range.
    fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        let len = self.len();
        let begin = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n + 1,
            Bound::Excluded(&n) => n,
            Bound::Unbounded => len,
        };
        assert!(begin <= end, "range start must not be greater than end");
        assert!(end <= len, "range end out of bounds");

        let mut builder = Self::Builder::with_capacity(end - begin);
        for i in begin..end {
            builder.push(self.get(i));
        }
        builder.finish()
    }
}

pub type NullArray = PrimitiveArray<()>;
pub type BoolArray = PrimitiveArray<bool>;
pub type I16Array = PrimitiveArray<i16>;
pub type I32Array = PrimitiveArray<i32>;
pub type I64Array = PrimitiveArray<i64>;
pub type F32Array = PrimitiveArray<F32>;
pub type F64Array = PrimitiveArray<F64>;
pub type DecimalArray = PrimitiveArray<Decimal>;
pub type DateArray = PrimitiveArray<Date>;
pub type TimestampArray = PrimitiveArray<Timestamp>;
pub type TimestampTzArray = PrimitiveArray<TimestampTz>;
pub type IntervalArray = PrimitiveArray<Interval>;

/// Embeds all types of arrays in `array` module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArrayImpl {
    Null(Arc<NullArray>),
    Bool(Arc<BoolArray>),
    Int16(Arc<I16Array>),
    Int32(Arc<I32Array>),
    Int64(Arc<I64Array>),
    // Float32(PrimitiveArray<f32>),
    Float64(Arc<F64Array>),
    String(Arc<StringArray>),
    Blob(Arc<BlobArray>),
    Vector(Arc<VectorArray>),
    Decimal(Arc<DecimalArray>),
    Date(Arc<DateArray>),
    Timestamp(Arc<TimestampArray>),
    TimestampTz(Arc<TimestampTzArray>),
    Interval(Arc<IntervalArray>),
}

pub type NullArrayBuilder = PrimitiveArrayBuilder<()>;
pub type BoolArrayBuilder = PrimitiveArrayBuilder<bool>;
pub type I16ArrayBuilder = PrimitiveArrayBuilder<i16>;
pub type I32ArrayBuilder = PrimitiveArrayBuilder<i32>;
pub type I64ArrayBuilder = PrimitiveArrayBuilder<i64>;
pub type F32ArrayBuilder = PrimitiveArrayBuilder<F32>;
pub type F64ArrayBuilder = PrimitiveArrayBuilder<F64>;
pub type DecimalArrayBuilder = PrimitiveArrayBuilder<Decimal>;
pub type DateArrayBuilder = PrimitiveArrayBuilder<Date>;
pub type TimestampArrayBuilder = PrimitiveArrayBuilder<Timestamp>;
pub type TimestampTzArrayBuilder = PrimitiveArrayBuilder<TimestampTz>;
pub type IntervalArrayBuilder = PrimitiveArrayBuilder<Interval>;

/// Embeds all types of array builders in `array` module.
pub enum ArrayBuilderImpl {
    Null(NullArrayBuilder),
    Bool(BoolArrayBuilder),
    Int16(I16ArrayBuilder),
    Int32(I32ArrayBuilder),
    Int64(I64ArrayBuilder),
    // Float32(PrimitiveArrayBuilder<f32>),
    Float64(F64ArrayBuilder),
    String(StringArrayBuilder),
    Blob(BlobArrayBuilder),
    Vector(VectorArrayBuilder),
    Decimal(DecimalArrayBuilder),
    Date(DateArrayBuilder),
    Timestamp(TimestampArrayBuilder),
    TimestampTz(TimestampTzArrayBuilder),
    Interval(IntervalArrayBuilder),
}

/// `for_all_variants` includes all variants of our array types. If you added a new array
/// type inside the project, be sure to add a variant here.
///
/// Every tuple has four elements, where
/// `{ enum variant name, function suffix name, array type, builder type, scalar type }`
///
/// There are typically two ways of using this macro, pass token or pass no token.
/// See the following implementations for example.
#[macro_export]
macro_rules! for_all_variants {
    ($macro:tt $(, $x:tt)*) => {
        $macro! {
            [$($x),*],
            { Null, (), null, NullArray, NullArrayBuilder, Null, Null },
            { Bool, bool, bool, BoolArray, BoolArrayBuilder, Bool, Bool },
            { Int16, i16, int16, I16Array, I16ArrayBuilder, Int16, Int16 },
            { Int32, i32, int32, I32Array, I32ArrayBuilder, Int32, Int32 },
            { Int64, i64, int64, I64Array, I64ArrayBuilder, Int64, Int64 },
            { Float64, F64, float64, F64Array, F64ArrayBuilder, Float64, Float64 },
            { Decimal, Decimal, decimal, DecimalArray, DecimalArrayBuilder, Decimal, Decimal(_, _) },
            { Date, Date, date, DateArray, DateArrayBuilder, Date, Date },
            { Timestamp, Timestamp, timestamp, TimestampArray, TimestampArrayBuilder, Timestamp, Timestamp },
            { TimestampTz, TimestampTz, timestamp_tz, TimestampTzArray, TimestampTzArrayBuilder, TimestampTz, TimestampTz },
            { Interval, Interval, interval, IntervalArray, IntervalArrayBuilder, Interval, Interval },
            { String, str, string, StringArray, StringArrayBuilder, String, String },
            { Blob, BlobRef, blob, BlobArray, BlobArrayBuilder, Blob, Blob },
            { Vector, VectorRef, vector, VectorArray, VectorArrayBuilder, Vector, Vector }
        }
    };
}

#[macro_export]
macro_rules! for_all_variants_without_null {
    ($macro:tt $(, $x:tt)*) => {
        $macro! {
            [$($x),*],
            { Bool, bool, bool, BoolArray, BoolArrayBuilder, Bool, Bool },
            { Int16, i16, int16, I16Array, I16ArrayBuilder, Int16, Int16 },
            { Int32, i32, int32, I32Array, I32ArrayBuilder, Int32, Int32 },
            { Int64, i64, int64, I64Array, I64ArrayBuilder, Int64, Int64 },
            { Float64, F64, float64, F64Array, F64ArrayBuilder, Float64, Float64 },
            { Decimal, Decimal, decimal, DecimalArray, DecimalArrayBuilder, Decimal, Decimal(_, _) },
            { Date, Date, date, DateArray, DateArrayBuilder, Date, Date },
            { Timestamp, Timestamp, timestamp, TimestampArray, TimestampArrayBuilder, Timestamp, Timestamp },
            { TimestampTz, TimestampTz, timestamp_tz, TimestampTzArray, TimestampTzArrayBuilder, TimestampTz, TimestampTz },
            { Interval, Interval, interval, IntervalArray, IntervalArrayBuilder, Interval, Interval },
            { String, str, string, StringArray, StringArrayBuilder, String, String },
            { Blob, BlobRef, blob, BlobArray, BlobArrayBuilder, Blob, Blob },
            { Vector, VectorRef, vector, VectorArray, VectorArrayBuilder, Vector, Vector(_) }
        }
    };
}

/// An error which can be returned when downcasting an [`ArrayImpl`] into a concrete type array.
#[derive(Debug, Clone)]
pub struct TypeMismatch;

/// Implement `From` and `TryFrom` between conversions of concrete array types and enum sum type.
macro_rules! impl_from {
    ([], $( { $Abc:ident, $Type:ty, $abc:ident, $AbcArray:ty, $AbcArrayBuilder:ty, $Value:ident, $Pattern:pat } ),*) => {
        $(
            /// Implement `AbcArray -> ArrayImpl`
            impl From<$AbcArray> for ArrayImpl {
                fn from(array: $AbcArray) -> Self {
                    Self::$Abc(array.into())
                }
            }

            /// Implement `ArrayImpl -> AbcArray`
            impl TryFrom<ArrayImpl> for Arc<$AbcArray> {
                type Error = TypeMismatch;

                fn try_from(array: ArrayImpl) -> Result<Self, Self::Error> {
                    match array {
                        ArrayImpl::$Abc(array) => Ok(array),
                        _ => Err(TypeMismatch),
                    }
                }
            }

            /// Implement `&ArrayImpl -> &AbcArray`
            impl<'a> TryFrom<&'a ArrayImpl> for &'a $AbcArray {
                type Error = TypeMismatch;

                fn try_from(array: &'a ArrayImpl) -> Result<Self, Self::Error> {
                    match array {
                        ArrayImpl::$Abc(array) => Ok(array),
                        _ => Err(TypeMismatch),
                    }
                }
            }

            /// Implement `AbcArrayBuilder -> ArrayBuilderImpl`
            impl From<$AbcArrayBuilder> for ArrayBuilderImpl {
                fn from(array: $AbcArrayBuilder) -> Self {
                    Self::$Abc(array)
                }
            }

            /// Implement `ArrayBuilderImpl -> AbcBuilder`
            impl TryFrom<ArrayBuilderImpl> for $AbcArrayBuilder {
                type Error = TypeMismatch;

                fn try_from(array: ArrayBuilderImpl) -> Result<Self, Self::Error> {
                    match array {
                        ArrayBuilderImpl::$Abc(array) => Ok(array),
                        _ => Err(TypeMismatch),
                    }
                }
            }

            /// Implement `&ArrayBuilderImpl -> &AbcBuilder`
            impl<'a> TryFrom<&'a ArrayBuilderImpl> for &'a $AbcArrayBuilder {
                type Error = TypeMismatch;

                fn try_from(array: &'a ArrayBuilderImpl) -> Result<Self, Self::Error> {
                    match array {
                        ArrayBuilderImpl::$Abc(array) => Ok(array),
                        _ => Err(TypeMismatch),
                    }
                }
            }
        )*
    };
}

for_all_variants! { impl_from }

/// Implement dispatch functions for `ArrayBuilderImpl`.
macro_rules! impl_array_builder {
    ([], $( { $Abc:ident, $Type:ty, $abc:ident, $AbcArray:ty, $AbcArrayBuilder:ty, $Value:ident, $Pattern:pat } ),*) => {
        impl ArrayBuilderImpl {
            /// Reserve at least `capacity` values.
            pub fn reserve(&mut self, capacity: usize) {
                match self {
                    Self::Null(a) => a.reserve(capacity),
                    $(
                        Self::$Abc(a) => a.reserve(capacity),
                    )*
                }
            }

            /// Create a new array builder with the same type of given array.
            pub fn from_type_of_array(array: &ArrayImpl) -> Self {
                match array {
                    ArrayImpl::Null(_) => Self::Null(NullArrayBuilder::new()),
                    $(
                        ArrayImpl::$Abc(_) => Self::$Abc(<$AbcArrayBuilder>::new()),
                    )*
                }
            }

            /// Create a new array builder with data type
            pub fn with_capacity(capacity: usize, ty: &DataType) -> Self {
                use DataType::*;
                match ty {
                    Null => Self::Null(NullArrayBuilder::with_capacity(capacity)),
                    Struct(_) => todo!("array of Struct type"),
                    $(
                        $Pattern => Self::$Abc(<$AbcArrayBuilder>::with_capacity(capacity)),
                    )*
                }
            }

            /// Appends an element to the back of array.
            pub fn push(&mut self, v: &DataValue) {
                match (self, v) {
                    (Self::Null(a), DataValue::Null) => a.push(None),
                    $(
                        (Self::$Abc(a), DataValue::$Value(v)) => a.push(Some(v)),
                        (Self::$Abc(a), DataValue::Null) => a.push(None),
                    )*
                    (b, v) => panic!("failed to push value: type mismatch. builder: {}, value: {:?}", b.type_string(), v),
                }
            }

            /// Appends an element `n` times to the back of array.
            pub fn push_n(&mut self, n: usize, v: &DataValue) {
                match (self, v) {
                    (Self::Null(a), DataValue::Null) => a.push_n(n, None),
                    $(
                        (Self::$Abc(a), DataValue::$Value(v)) => a.push_n(n, Some(v)),
                        (Self::$Abc(a), DataValue::Null) => a.push_n(n, None),
                    )*
                    (b, v) => panic!("failed to push value: type mismatch. builder: {}, value: {:?}", b.type_string(), v),
                }
            }

            /// Take all elements to a new array.
            pub fn take(&mut self) -> ArrayImpl {
                match self {
                    Self::Null(a) => ArrayImpl::Null(a.take().into()),
                    $(
                        Self::$Abc(a) => ArrayImpl::$Abc(a.take().into()),
                    )*
                }
            }

            /// Finish build and return a new array.
            pub fn finish(self) -> ArrayImpl {
                match self {
                    Self::Null(a) => ArrayImpl::Null(a.finish().into()),
                    $(
                        Self::$Abc(a) => ArrayImpl::$Abc(a.finish().into()),
                    )*
                }
            }

            /// Appends an `ArrayImpl`
            pub fn append(&mut self, array_impl: &ArrayImpl) {
                match (self, array_impl) {
                    (Self::Null(builder), ArrayImpl::Null(arr)) => builder.append(arr),
                    $(
                        (Self::$Abc(builder), ArrayImpl::$Abc(arr)) => builder.append(arr),
                    )*
                    (b, a) => panic!("failed to append array: type mismatch. builder: {}, array: {}", b.type_string(), a.type_string()),
                }
            }

            /// Return a string describing the type of this array.
            fn type_string(&self) -> &'static str {
                match self {
                    Self::Null(_) => "NULL",
                    $(
                        Self::$Abc(_) => stringify!($Abc),
                    )*
                }
            }
        }
    }
}

for_all_variants_without_null! { impl_array_builder }

impl ArrayBuilderImpl {
    /// Create a new array builder from data type.
    pub fn new(ty: &DataType) -> Self {
        Self::with_capacity(0, ty)
    }

    /// Appends an element in string.
    pub fn push_str(&mut self, s: &str) -> Result<(), ConvertError> {
        let null = s.is_empty();
        match self {
            Self::Null(a) => a.push(None),
            Self::Bool(a) if null => a.push(None),
            Self::Int16(a) if null => a.push(None),
            Self::Int32(a) if null => a.push(None),
            Self::Int64(a) if null => a.push(None),
            Self::Float64(a) if null => a.push(None),
            Self::String(a) if null => a.push(None),
            Self::Blob(a) if null => a.push(None),
            Self::Vector(a) if null => a.push(None),
            Self::Decimal(a) if null => a.push(None),
            Self::Date(a) if null => a.push(None),
            Self::Timestamp(a) if null => a.push(None),
            Self::TimestampTz(a) if null => a.push(None),
            Self::Interval(a) if null => a.push(None),
            Self::Bool(a) => a.push(Some(
                &s.parse::<bool>()
                    .map_err(|e| ConvertError::ParseBool(s.to_string(), e))?,
            )),
            Self::Int16(a) => a.push(Some(
                &s.parse::<i16>()
                    .map_err(|e| ConvertError::ParseInt(s.to_string(), e))?,
            )),
            Self::Int32(a) => a.push(Some(
                &s.parse::<i32>()
                    .map_err(|e| ConvertError::ParseInt(s.to_string(), e))?,
            )),
            Self::Int64(a) => a.push(Some(
                &s.parse::<i64>()
                    .map_err(|e| ConvertError::ParseInt(s.to_string(), e))?,
            )),
            Self::Float64(a) => a.push(Some(
                &s.parse::<F64>()
                    .map_err(|e| ConvertError::ParseFloat(s.to_string(), e))?,
            )),
            Self::String(a) => a.push(Some(s)),
            Self::Blob(a) => a.push(Some(
                &s.parse::<Blob>()
                    .map_err(|e| ConvertError::ParseBlob(s.to_string(), e))?,
            )),
            Self::Vector(a) => a.push(Some(
                &s.parse::<Vector>()
                    .map_err(|e| ConvertError::ParseVector(s.to_string(), e))?,
            )),
            Self::Decimal(a) => a.push(Some(
                &Decimal::from_str(s).map_err(|e| ConvertError::ParseDecimal(s.to_string(), e))?,
            )),
            Self::Date(a) => a.push(Some(
                &Date::from_str(s).map_err(|e| ConvertError::ParseDate(s.to_string(), e))?,
            )),
            Self::Timestamp(a) => a
                .push(Some(&Timestamp::from_str(s).map_err(|e| {
                    ConvertError::ParseTimestamp(s.to_string(), e)
                })?)),
            Self::TimestampTz(a) => a
                .push(Some(&TimestampTz::from_str(s).map_err(|e| {
                    ConvertError::ParseTimestampTz(s.to_string(), e)
                })?)),
            Self::Interval(a) => a.push(Some(
                &Interval::from_str(s)
                    .map_err(|e| ConvertError::ParseInterval(s.to_string(), e))?,
            )),
        }
        Ok(())
    }
}

/// Implement dispatch functions for `ArrayImpl`.
macro_rules! impl_array {
    ([], $( { $Abc:ident, $Type:ty, $abc:ident, $AbcArray:ty, $AbcArrayBuilder:ty, $Value:ident, $Pattern:pat } ),*) => {
        impl ArrayImpl {
            $(
                paste! {
                    /// Create a new array of the corresponding type
                    pub fn [<new_ $abc>](array: $AbcArray) -> Self {
                        ArrayImpl::$Abc(array.into())
                    }
                }
            )*

            /// Create a new array of the corresponding type
            pub fn new_null(array: NullArray) -> Self {
                ArrayImpl::Null(array.into())
            }

            /// Get the value and convert it to string.
            pub fn get_to_string(&self, idx: usize) -> String {
                match self {
                    Self::Null(_) => None,
                    $(
                        Self::$Abc(a) => a.get(idx).map(|v| v.to_string()),
                    )*
                }
                .unwrap_or_else(|| "NULL".into())
            }

            /// Get the value at the given index.
            pub fn get(&self, idx: usize) -> DataValue {
                match self {
                    Self::Null(_) => DataValue::Null,
                    $(
                        Self::$Abc(a) => match a.get(idx) {
                            Some(val) => DataValue::$Value(val.to_owned().into()),
                            None => DataValue::Null,
                        },
                    )*
                }
            }

            /// Get iterator of current array.
            pub fn iter(&self) -> impl DoubleEndedIterator<Item = DataValue> + '_ {
                (0..self.len()).map(|i| self.get(i))
            }

            /// Number of items of array.
            pub fn len(&self) -> usize {
                match self {
                    Self::Null(a) => a.len(),
                    $(
                        Self::$Abc(a) => a.len(),
                    )*
                }
            }

            /// Filter the elements and return a new array.
            pub fn filter(&self, visibility: &[bool]) -> Self {
                match self {
                    Self::Null(a) => Self::Null(a.filter(&visibility).into()),
                    $(
                        Self::$Abc(a) => Self::$Abc(a.filter(&visibility).into()),
                    )*
                }
            }

            /// Return a slice of self for the provided range.
            pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
                match self {
                    Self::Null(a) => Self::Null(a.slice(range).into()),
                    $(
                        Self::$Abc(a) => Self::$Abc(a.slice(range).into()),
                    )*
                }
            }

            /// Return a string describing the type of this array.
            pub fn type_string(&self) -> &'static str {
                match self {
                    Self::Null(_) => "NULL",
                    $(
                        Self::$Abc(_) => stringify!($Abc),
                    )*
                }
            }
        }
    }
}

for_all_variants_without_null! { impl_array }

impl ArrayImpl {
    /// Check if array is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Create a single element array from data value.
impl From<&DataValue> for ArrayImpl {
    fn from(val: &DataValue) -> Self {
        match val {
            DataValue::Null => Self::new_null([None].into_iter().collect()),
            &DataValue::Bool(v) => Self::new_bool([v].into_iter().collect()),
            &DataValue::Int16(v) => Self::new_int16([v].into_iter().collect()),
            &DataValue::Int32(v) => Self::new_int32([v].into_iter().collect()),
            &DataValue::Int64(v) => Self::new_int64([v].into_iter().collect()),
            &DataValue::Float64(v) => Self::new_float64([v].into_iter().collect()),
            DataValue::String(v) => Self::new_string([Some(v)].into_iter().collect()),
            DataValue::Blob(v) => Self::new_blob([Some(v)].into_iter().collect()),
            &DataValue::Decimal(v) => Self::new_decimal([v].into_iter().collect()),
            &DataValue::Date(v) => Self::new_date([v].into_iter().collect()),
            &DataValue::Timestamp(v) => Self::new_timestamp([v].into_iter().collect()),
            &DataValue::TimestampTz(v) => Self::new_timestamp_tz([v].into_iter().collect()),
            &DataValue::Interval(v) => Self::new_interval([v].into_iter().collect()),
            DataValue::Vector(v) => Self::new_vector([Some(v)].into_iter().collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter() {
        let array: PrimitiveArray<i32> = (0..=60).map(Some).collect();
        let array: PrimitiveArray<i32> = array
            .iter()
            .filter(|x| *x.unwrap_or(&0) >= 60)
            .map(|x| x.cloned())
            .collect();
        assert_eq!(
            array.iter().map(|x| x.cloned()).collect::<Vec<_>>(),
            vec![Some(60)]
        );
    }

    use num_traits::cast::AsPrimitive;
    use num_traits::ops::checked::CheckedAdd;

    use crate::types::NativeType;

    fn vec_add<T1, T2, T3>(a: &PrimitiveArray<T1>, b: &PrimitiveArray<T2>) -> PrimitiveArray<T3>
    where
        T1: NativeType + AsPrimitive<T3>,
        T2: NativeType + AsPrimitive<T3>,
        T3: NativeType + CheckedAdd,
    {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b.iter())
            .map(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Some(a.as_() + b.as_()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_vectorized_add() {
        let array1 = (0i32..=60).map(Some).collect();
        let array2 = (0i16..=60).map(Some).collect();

        let final_array = vec_add(&array1, &array2) as PrimitiveArray<i64>;
        assert_eq!(
            final_array.iter().map(|x| x.cloned()).collect::<Vec<_>>(),
            (0i64..=60).map(|i| Some(i * 2)).collect::<Vec<_>>()
        );
    }
}
