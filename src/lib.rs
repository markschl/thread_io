extern crate crossbeam;

pub mod read;
pub mod write;

fn unwrap_or_resume_unwind<V>(value: Result<V, Box<dyn std::any::Any + Send>>) -> V {
    match value {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error),
    }
}
