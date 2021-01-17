use crate::res::L;
use crate::RollbackBuffer;
use bevy::ecs::{WorldQuery, QueryFilter, World, TypeAccess, ArchetypeComponent,
    Batch, BatchedIter, QueryError, Entity, Component, Mut, ReadOnlyFetch,
    QueryIter, Fetch, ComponentError, SystemParam, FetchSystemParam, SystemState,
    Resources, QueryAccess, ResourceIndex};
use bevy::tasks::{ParallelIterator};
use std::marker::PhantomData;
use std::any::TypeId;
use std::ops::Deref;
use std::ptr::NonNull;

/// Provides scoped access to a World according to a given [HecsQuery]
pub struct LQuery<'a, Q: WorldQuery, F: QueryFilter = ()> {
    pub(crate) world: NonNull<RollbackBuffer>,
    pub(crate) component_access: &'a TypeAccess<ArchetypeComponent>,
    _marker: PhantomData<(Q, F)>,
}


impl<'a, Q: WorldQuery, F: QueryFilter> LQuery<'a, Q, F> {
    /// # Safety
    /// This will create a Query that could violate memory safety rules. Make sure that this is only called in
    /// ways that ensure the Queries have unique mutable access.
    #[inline]
    pub(crate) unsafe fn new(
        world: NonNull<RollbackBuffer>,
        component_access: &'a TypeAccess<ArchetypeComponent>,
    ) -> Self {
        Self {
            world,
            component_access,
            _marker: PhantomData::default(),
        }
    }

    /// Iterates over the query results. This can only be called for read-only queries
    #[inline]
    pub fn iter(&self) -> QueryIter<'_, Q, F>
    where
        Q::Fetch: ReadOnlyFetch,
    {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe { self.world.as_ref().current_world.query_unchecked() }
    }

    /// Iterates over the query results
    #[inline]
    pub fn iter_mut(&mut self) -> QueryIter<'_, Q, F> {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe { self.world.as_ref().current_world.query_unchecked() }
    }

    /// Iterates over the query results
    /// # Safety
    /// This allows aliased mutability. You must make sure this call does not result in multiple mutable references to the same component
    #[inline]
    pub unsafe fn iter_unsafe(&self) -> QueryIter<'_, Q, F> {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        self.world.as_ref().current_world.query_unchecked()
    }

    #[inline]
    pub fn par_iter(&self, batch_size: usize) -> ParIter<'_, Q, F>
    where
        Q::Fetch: ReadOnlyFetch,
    {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe { ParIter::new(self.world.as_ref().current_world.query_batched_unchecked(batch_size)) }
    }

    #[inline]
    pub fn par_iter_mut(&mut self, batch_size: usize) -> ParIter<'_, Q, F> {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe { ParIter::new(self.world.as_ref().current_world.query_batched_unchecked(batch_size)) }
    }

    /// Gets the query result for the given `entity`
    #[inline]
    pub fn get(&self, entity: Entity) -> Result<<Q::Fetch as Fetch>::Item, QueryError>
    where
        Q::Fetch: ReadOnlyFetch,
    {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe {
            self.world
                .as_ref()
                .current_world
                .query_one_unchecked::<Q, F>(entity)
                .map_err(|_err| QueryError::NoSuchEntity)
        }
    }

    /// Gets the query result for the given `entity`
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Result<<Q::Fetch as Fetch>::Item, QueryError> {
        // SAFE: system runs without conflicts with other systems. same-system queries have runtime borrow checks when they conflict
        unsafe {
            self.world
                .as_ref()
                .current_world
                .query_one_unchecked::<Q, F>(entity)
                .map_err(|_err| QueryError::NoSuchEntity)
        }
    }

    /// Gets the query result for the given `entity`
    /// # Safety
    /// This allows aliased mutability. You must make sure this call does not result in multiple mutable references to the same component
    #[inline]
    pub unsafe fn get_unsafe(
        &self,
        entity: Entity,
    ) -> Result<<Q::Fetch as Fetch>::Item, QueryError> {
        self.world
            .as_ref()
            .current_world
            .query_one_unchecked::<Q, F>(entity)
            .map_err(|_err| QueryError::NoSuchEntity)
    }

    /// Gets a reference to the entity's component of the given type. This will fail if the entity does not have
    /// the given component type or if the given component type does not match this query.
    pub fn get_component<T: Component>(&self, entity: Entity) -> Result<&T, QueryError> {
        if let Some(location) = unsafe{self.world.as_ref().current_world.get_entity_location(entity)} {
            if self
                .component_access
                .is_read_or_write(&ArchetypeComponent::new::<L<T>>(location.archetype))
            {
                // SAFE: we have already checked that the entity/component matches our archetype access. and systems are scheduled to run with safe archetype access
                unsafe {
                    self.world
                        .as_ref()
                        .current_world
                        .get_at_location_unchecked(location)
                        .map_err(QueryError::ComponentError)
                }
            } else {
                Err(QueryError::CannotReadArchetype)
            }
        } else {
            Err(QueryError::ComponentError(ComponentError::NoSuchEntity))
        }
    }

    /// Gets a mutable reference to the entity's component of the given type. This will fail if the entity does not have
    /// the given component type or if the given component type does not match this query.
    pub fn get_component_mut<T: Component>(
        &mut self,
        entity: Entity,
    ) -> Result<Mut<'_, T>, QueryError> {
        let location = unsafe{match self.world.as_ref().current_world.get_entity_location(entity) {
            None => return Err(QueryError::ComponentError(ComponentError::NoSuchEntity)),
            Some(location) => location,
        }};

        if self
            .component_access
            .is_write(&ArchetypeComponent::new::<L<T>>(location.archetype))
        {
            // SAFE: RefMut does exclusivity checks and we have already validated the entity
            unsafe {
                self.world
                    .as_ref()
                    .current_world
                    .get_mut_at_location_unchecked(location)
                    .map_err(QueryError::ComponentError)
            }
        } else {
            Err(QueryError::CannotWriteArchetype)
        }
    }

    /// Gets a mutable reference to the entity's component of the given type. This will fail if the entity does not have
    /// the given component type
    /// # Safety
    /// This allows aliased mutability. You must make sure this call does not result in multiple mutable references to the same component
    pub unsafe fn get_component_unsafe<T: Component>(
        &self,
        entity: Entity,
    ) -> Result<Mut<'_, T>, QueryError> {
        self.world
            .as_ref()
            .current_world
            .get_mut_unchecked(entity)
            .map_err(QueryError::ComponentError)
    }

    /// Returns an array containing the `Entity`s in this `Query` that had the given `Component`
    /// removed in this update.
    ///
    /// `removed::<C>()` only returns entities whose components were removed before the
    /// current system started.
    ///
    /// Regular systems do not apply `Commands` until the end of their stage. This means component
    /// removals in a regular system won't be accessible through `removed::<C>()` in the same
    /// stage, because the removal hasn't actually occurred yet. This can be solved by executing
    /// `removed::<C>()` in a later stage. `AppBuilder::add_system_to_stage()` can be used to
    /// control at what stage a system runs.
    ///
    /// Thread local systems manipulate the world directly, so removes are applied immediately. This
    /// means any system that runs after a thread local system in the same update will pick up
    /// removals that happened in the thread local system, regardless of stages.
    pub fn removed<C: Component>(&self) -> &[Entity] {
        unsafe{self.world.as_ref().current_world.removed::<C>()}
    }

    /// Sets the entity's component to the given value. This will fail if the entity does not already have
    /// the given component type or if the given component type does not match this query.
    pub fn set<T: Component>(&mut self, entity: Entity, component: T) -> Result<(), QueryError> {
        let mut current = self.get_component_mut::<T>(entity)?;
        *current = component;
        Ok(())
    }
}

/// Parallel version of QueryIter
pub struct ParIter<'w, Q: WorldQuery, F: QueryFilter> {
    batched_iter: BatchedIter<'w, Q, F>,
}

impl<'w, Q: WorldQuery, F: QueryFilter> ParIter<'w, Q, F> {
    pub fn new(batched_iter: BatchedIter<'w, Q, F>) -> Self {
        Self { batched_iter }
    }
}

unsafe impl<'w, Q: WorldQuery, F: QueryFilter> Send for ParIter<'w, Q, F> {}

impl<'w, Q: WorldQuery, F: QueryFilter> ParallelIterator<Batch<'w, Q, F>> for ParIter<'w, Q, F> {
    type Item = <Q::Fetch as Fetch<'w>>::Item;

    #[inline]
    fn next_batch(&mut self) -> Option<Batch<'w, Q, F>> {
        self.batched_iter.next()
    }
}

pub struct FetchLQuery<Q, F>(PhantomData<(Q, F)>);

impl<'a, Q: WorldQuery, F: QueryFilter> SystemParam for LQuery<'a, Q, F> {
    type Fetch = FetchLQuery<Q, F>;
}

impl<'a, Q: WorldQuery, F: QueryFilter> FetchSystemParam<'a> for FetchLQuery<Q, F> {
    type Item = LQuery<'a, Q, F>;

    #[inline]
    unsafe fn get_param(
        system_state: &'a SystemState,
        world: &'a World,
        resources: &'a Resources,
    ) -> Option<Self::Item> {
        let query_index = *system_state.current_query_index.get();
        let archetype_component_access: &'a TypeAccess<ArchetypeComponent> =
            &system_state.query_archetype_component_accesses[query_index];
        *system_state.current_query_index.get() += 1;
        
        if system_state.resource_access.is_write(&TypeId::of::<RollbackBuffer>()){
            panic!(
                "System '{}' is trying to access Logical Resources while mutating the RollbackBuffer!",
                system_state.name
            );
        }

        let world = resources
            .get_unsafe_ref::<RollbackBuffer>(ResourceIndex::Global);
        Some(LQuery::new(
            world,
            archetype_component_access))
    }

    fn init(system_state: &mut SystemState, _world: &World, _resources: &mut Resources) {
        system_state
            .query_archetype_component_accesses
            .push(TypeAccess::default());
        let access = QueryAccess::union(vec![Q::Fetch::access(), F::access()]);
        system_state.query_accesses.push(vec![access]);
        system_state
            .query_type_names
            .push(std::any::type_name::<L<Q>>());
    }
}