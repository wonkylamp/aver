use std::collections::HashMap;
use std::sync::Arc;

use aver_core::{Id, Object};
use parking_lot::Mutex;

use crate::store::{Store, StoreError};

/// A bounded cache of parsed immutable objects keyed by content id, so it
/// never needs invalidation. Eviction is generational, an O(1) approximation
/// of LRU: hits move an entry to the young generation, and when the young
/// generation fills it becomes the old one and the previous old one is dropped.
pub(super) struct ObjectCache<T: Object> {
    capacity: usize,
    generations: Mutex<Generations<T>>,
}

struct Generations<T: Object> {
    young: HashMap<Id<T::Kind>, Arc<T>>,
    old: HashMap<Id<T::Kind>, Arc<T>>,
}

impl<T: Object> ObjectCache<T> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(2),
            generations: Mutex::new(Generations {
                young: HashMap::new(),
                old: HashMap::new(),
            }),
        }
    }

    pub(super) fn get(&self, store: &Store, id: &Id<T::Kind>) -> Result<Arc<T>, StoreError> {
        {
            let mut gens = self.generations.lock();
            if let Some(hit) = gens.young.get(id) {
                return Ok(hit.clone());
            }
            if let Some(hit) = gens.old.remove(id) {
                self.insert(&mut gens, *id, hit.clone());
                return Ok(hit);
            }
        }
        let object = Arc::new(store.get::<T>(id)?);
        self.insert(&mut self.generations.lock(), *id, object.clone());
        Ok(object)
    }

    fn insert(&self, gens: &mut Generations<T>, id: Id<T::Kind>, object: Arc<T>) {
        if gens.young.len() >= self.capacity / 2 {
            gens.old = std::mem::take(&mut gens.young);
        }
        gens.young.insert(id, object);
    }
}
