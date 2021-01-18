use bevy::prelude::*;
use bevy::ecs::{Command, DynamicBundle, EntityReserver, SystemId, SystemStage, SystemState, FetchSystemParam, SystemParam};
use crate::RollbackBuffer;
use std::ops::DerefMut;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, Arc};
use std::any::TypeId;

pub struct LogicCommands{
    commands: Commands,
}

impl LogicCommands {
    pub fn spawn(&mut self, bundle: impl DynamicBundle + Send + Sync + 'static) -> &mut Self {
        self.commands.spawn(bundle);
        self
    }

    /// Equivalent to iterating `bundles_iter` and calling [`Self::spawn`] on each bundle, but slightly more performant.
    pub fn spawn_batch<I>(&mut self, bundles_iter: I) -> &mut Self
    where
        I: IntoIterator + Send + Sync + 'static,
        I::Item: Bundle,
    {
        self.commands.spawn_batch(bundles_iter);
        self
    }

    /// Despawns only the specified entity, not including its children.
    pub fn despawn(&mut self, entity: Entity) -> &mut Self {
        self.commands.despawn(entity);
        self
    }

    /// Inserts a bundle of components into `entity`.
    ///
    /// See [`World::insert`].
    pub fn insert(
        &mut self,
        entity: Entity,
        bundle: impl DynamicBundle + Send + Sync + 'static,
    ) -> &mut Self {
        self.commands.insert(entity, bundle);
        self
    }

    /// Inserts a single component into `entity`.
    ///
    /// See [`World::insert_one`].
    pub fn insert_one(&mut self, entity: Entity, component: impl Component) -> &mut Self {
        self.commands.insert_one(entity, component);
        self
    }

    pub fn insert_resource<T: Resource>(&mut self, resource: T) -> &mut Self {
        self.commands.insert_resource(resource);
        self
    }

    /// Insert a resource that is local to a specific system.
    ///
    /// See [`crate::System::id`].
    pub fn insert_local_resource<T: Resource>(
        &mut self,
        system_id: SystemId,
        resource: T,
    ) -> &mut Self {
        self.commands.insert_local_resource(
            system_id,
            resource
        );
        self
    }

    /// See [`World::remove_one`].
    pub fn remove_one<T>(&mut self, entity: Entity) -> &mut Self
    where
        T: Component,
    {
        self.commands.remove_one::<T>(
            entity
        );
        self
    }

    /// See [`World::remove`].
    pub fn remove<T>(&mut self, entity: Entity) -> &mut Self
    where
        T: Bundle + Send + Sync + 'static,
    {
        self.commands.remove::<T>(
            entity
        );
        self
    }

    /// Adds a bundle of components to the current entity.
    ///
    /// See [`Self::with`], [`Self::current_entity`].
    pub fn with_bundle(&mut self, bundle: impl DynamicBundle + Send + Sync + 'static) -> &mut Self {
        self.commands.with_bundle(
            bundle
        );
        self
    }

    pub fn with(&mut self, component: impl Component) -> &mut Self {
        self.commands.with(component);
        self
    }

    /// Adds a command directly to the command list. Prefer this to [`Self::add_command_boxed`] if the type of `command` is statically known.
    pub fn add_command<C: Command + 'static>(&mut self, command: C) -> &mut Self {
        self.commands.add_command(command);
        self
    }

    /// See [`Self::add_command`].
    pub fn add_command_boxed(&mut self, command: Box<dyn Command>) -> &mut Self {
        self.commands.add_command_boxed(command);
        self
    }

    /// Runs all the stored commands on `world` and `resources`. The command buffer is emptied as a part of this call.
    pub fn apply(&mut self, world: &mut World, resources: &mut Resources) {
        let mut rollback_buffer_r = resources
            .get_mut::<RollbackBuffer>()
            .expect("Couldn't find RollbackBuffer!");
        let mut rollback_buffer = rollback_buffer_r.deref_mut();
        let world = &mut rollback_buffer.current_world;
        let resources = &mut rollback_buffer.current_resources;
        self.commands.apply(world, resources);
    }

    /// Returns the current entity, set by [`Self::spawn`] or with [`Self::set_current_entity`].
    pub fn current_entity(&self) -> Option<Entity> {
        self.commands.current_entity()
    }

    pub fn set_current_entity(&mut self, entity: Entity) {
        self.commands.set_current_entity(entity);
    }

    pub fn clear_current_entity(&mut self) {
        self.commands.clear_current_entity();
    }

    pub fn for_current_entity(&mut self, f: impl FnOnce(Entity)) -> &mut Self {
        self.commands.for_current_entity(f);
        self
    }

    pub fn set_entity_reserver(&mut self, entity_reserver: EntityReserver) {
        self.commands.set_entity_reserver(entity_reserver);
    }
}

pub struct FetchLogicCommands;

impl<'a> SystemParam for &'a mut LogicCommands {
    type Fetch = FetchLogicCommands;
}

impl<'a> FetchSystemParam<'a> for FetchLogicCommands {
    type Item = &'a mut LogicCommands;

    fn init(system_state: &mut SystemState, world: &World, _resources: &mut Resources) {
        // SAFE: this is called with unique access to SystemState
        unsafe {
            (&mut *system_state.commands.get()).set_entity_reserver(world.get_entity_reserver())
        }
    }

    #[inline]
    unsafe fn get_param(
        system_state: &'a SystemState,
        _world: &'a World,
        _resources: &'a Resources,
    ) -> Option<Self::Item> {
        Some(&mut *system_state.commands.get())
    }
}

pub struct FetchArcLogicCommands;
impl SystemParam for Arc<Mutex<LogicCommands>> {
    type Fetch = FetchArcLogicCommands;
}

impl<'a> FetchSystemParam<'a> for FetchArcLogicCommands {
    type Item = Arc<Mutex<LogicCommands>>;

    fn init(system_state: &mut SystemState, world: &World, resources: &mut Resources) {
        if system_state.resource_access.is_write(&TypeId::of::<RollbackBuffer>()){
            panic!(
                "System '{}' is trying to access Logical Resources while mutating the RollbackBuffer!",
                system_state.name
            );
        };

        let rollback_buffer = resources
            .get::<RollbackBuffer>()
            .expect("Couldn't find RollbackBuffer");

        system_state.other_commands.get_or_insert(Vec::new());
        system_state.other_commands.push({
            let mut commands = LogicCommands::default();
            commands.set_entity_reserver(rollback_buffer.current_world.get_entity_reserver());
            Arc::new(Mutex::new(commands))
        });
    }

    #[inline]
    unsafe fn get_param(
        system_state: &SystemState,
        _world: &World,
        _resources: &Resources,
    ) -> Option<Self::Item> {
        Some(system_state.other_commands.get(system_state.other_commands_init.fetch_add(0, Ordering::SeqCst)))
    }
}