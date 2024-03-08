use crate::{trace::TracingStats, ObjectModel};

pub trait Tracer<O: ObjectModel> {
    fn startup(&self);
    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats;
    fn teardown(&self);
}
