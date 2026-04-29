use clap::ArgMatches;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Unified view over Phase 1 and Phase 2 clap matches.
/// Prefers Phase 2 values, falling back to Phase 1.
#[derive(Debug, Clone, Copy)]
pub struct MergedMatches<'a> {
    pub phase1: &'a ArgMatches,
    pub phase2: &'a ArgMatches,
}

impl<'a> MergedMatches<'a> {
    pub fn get_flag(&self, id: &str) -> bool {
        (catch_unwind(AssertUnwindSafe(|| self.phase2.get_flag(id))).unwrap_or(false))
            || (catch_unwind(AssertUnwindSafe(|| self.phase1.get_flag(id))).unwrap_or(false))
    }

    pub fn get_one<T: Clone + Send + Sync + 'static>(&self, id: &str) -> Option<T> {
        catch_unwind(AssertUnwindSafe(|| self.phase2.get_one::<T>(id).cloned()))
            .ok()
            .flatten()
            .or_else(|| {
                catch_unwind(AssertUnwindSafe(|| self.phase1.get_one::<T>(id).cloned()))
                    .ok()
                    .flatten()
            })
    }

    pub fn get_many<T: Clone + Send + Sync + 'static>(
        &self,
        id: &str,
    ) -> Option<clap::parser::ValuesRef<'a, T>> {
        catch_unwind(AssertUnwindSafe(|| self.phase2.get_many::<T>(id)))
            .ok()
            .flatten()
            .or_else(|| {
                catch_unwind(AssertUnwindSafe(|| self.phase1.get_many::<T>(id)))
                    .ok()
                    .flatten()
            })
    }
}
