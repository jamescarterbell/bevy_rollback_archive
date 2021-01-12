#![feature(trait_alias)]

use bevy::{
    ecs::{Schedule, Stage, ShouldRun, Archetype},
    prelude::{
        *,
        stage::UPDATE,
    },
    reflect::TypeRegistryArc,
    scene::serde::SceneSerializer,
};
use std::sync::Mutex;
use std::ops::DerefMut;
use std::collections::hash_map::*;

pub mod stage{
    pub const ROLLBACK_UPDATE: &str = "rollback_update";
}

pub mod logic_stages{
    pub const SAVE_STATE: &str = "save_state";
    pub const LOGIC_UPDATE: &str = "logic_update";
    pub const LOGIC_PREUPDATE: &str = "logic_preupdate";
    pub const LOGIC_POSTUPDATE: &str = "logic_postupdate";
}

pub struct RollbackPlugin{
    schedule: Mutex<Option<Schedule>>,
    buffer_size: usize,
    run_criteria: Mutex<Option<Box<dyn System<In = (), Out = ShouldRun>>>>,
}

impl RollbackPlugin{
    pub fn with_schedule(buffer_size: usize, schedule: Schedule) -> Self{
        Self{
            schedule: Mutex::new(Some(schedule)),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_buffer_size(buffer_size: usize) -> Self{
        Self{
            schedule: Mutex::new(None),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.run_criteria = Mutex::new(Some(Box::new(system)));
        self
    }
}

impl Plugin for RollbackPlugin{
    fn build(&self, app: &mut AppBuilder){
        let mut resources = app.resources_mut();

        let rollback_buffer = RollbackBuffer::new(
            self.buffer_size,
            &mut resources.get_mut::<Assets<DynamicScene>>().expect("Couldn't find DynamicScene!"),
            &resources.get::<TypeRegistryArc>().expect("Couldn't find TypeRegistryArc"),
        );

        drop(resources);

        let run_criteria = self.run_criteria.lock().unwrap().take();

        
        let mut stage = {
            if let Some(schedule) = self.schedule.lock().unwrap().take(){
                RollbackStage::with_schedule(schedule)
            }
            else{
                RollbackStage::new()
            }
        };
        
        stage.run_criteria = run_criteria;
        stage.run_criteria_initialized = false;

        app
            .add_resource(
                rollback_buffer
            )
            .add_stage_before(
                UPDATE,
                stage::ROLLBACK_UPDATE,
                stage
            );
    }
}

pub struct RollbackStage{
    schedule: Schedule,
    initialized: bool,
    run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    run_criteria_initialized: bool,
}

impl RollbackStage{
    pub fn with_schedule(schedule: Schedule) -> Self{
        Self{
            schedule,
            initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn new() -> Self{
        Self{
            schedule: Schedule::default()
                .with_stage(logic_stages::LOGIC_UPDATE, SystemStage::parallel())
                .with_stage_before(logic_stages::LOGIC_UPDATE, logic_stages::LOGIC_PREUPDATE, SystemStage::parallel())
                .with_stage_after(logic_stages::LOGIC_UPDATE, logic_stages::LOGIC_POSTUPDATE, SystemStage::parallel()),
            initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn run_once(&mut self, world: &mut World, resources: &mut Resources){
        let mut rollback_buffer = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");

        let state = rollback_buffer.newest_frame;
        let target = state % rollback_buffer.past_resources.len();
        // Apply changes
        if let Some(changes) = rollback_buffer.buffered_changes.remove(&state){
            changes.run_once(
                &mut rollback_buffer.current_world,
                &mut rollback_buffer.current_resources,
            );
        }

        // Apply overrides
        for override_fn in rollback_buffer.resource_override.iter(){
            (override_fn)(&mut rollback_buffer.current_resources, rollback_buffer.past_resources.get(target).unwrap().as_ref().unwrap());
        }
        
        // Run the schedule
        self.schedule.run_once(&mut rollback_buffer.current_world, &mut rollback_buffer.current_resources);
        // Store the new stuff
        store_new_resources(resources);
        store_new_world(resources);
    }

    pub fn run_rollback(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            
            let current_state = resources
                .get::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!")
                .rollback_state
                .clone();

            match current_state{
                RollbackState::Rollback(state) => {
                    // Literally just swap the worlds
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");

                    let target = rollback_buffer.newest_frame % 
                        rollback_buffer
                            .past_worlds
                            .len();

                    rollback_buffer
                        .current_world = rollback_buffer
                            .past_worlds
                            .get_mut(target)
                            .unwrap()
                            .take()
                            .expect("Frame doesn't exist!");

                    rollback_buffer
                        .current_resources = rollback_buffer
                            .past_resources
                            .get_mut(target)
                            .unwrap()
                            .take()
                            .expect("Frame doesn't exist!");

                    // Setup for catchup
                    rollback_buffer
                        .rollback_state = RollbackState::Rolledback(state);
                },
                RollbackState::Rolledback(state) => {
                    // Run schedule for state_n
                    self.run_once(world, resources);
                    
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");
            
                    // Increment counters
                    let new_state = state + 1;
                    rollback_buffer.rollback_state = RollbackState::Rolledback(new_state);

                    if new_state == rollback_buffer.newest_frame{
                        // We're all caugt up!
                        break;
                    }
                }
            };
        };
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.set_run_criteria(system);
        self
    }

    pub fn set_run_criteria<S: System<In = (), Out = ShouldRun>>(
        &mut self,
        system: S,
    ) -> &mut Self {
        self.run_criteria = Some(Box::new(system.system()));
        self.run_criteria_initialized = false;
        self
    }
}

impl Stage for RollbackStage{
    fn initialize(&mut self, world: &mut World, resources: &mut Resources){
        if let Some(ref mut run_criteria) = self.run_criteria{
            if !self.run_criteria_initialized{
                run_criteria.initialize(world, resources);
                self.run_criteria_initialized = true;
            }
        }

        if !self.initialized{
            self.initialized = true;

            let mut rollback_buffer = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");

            let mut scene_spawner = resources
                .get_mut::<SceneSpawner>()
                .expect("Couldn't find SceneSpawner!");

            scene_spawner
                .spawn_dynamic_sync(
                    world,
                    resources,
                    &rollback_buffer.tracked_entities
                );

            let resource_override_fn = rollback_buffer.resource_override_fn.take().unwrap_or(Vec::new());

            let mut past_resources = Resources::default();

            drop(rollback_buffer);
            drop(scene_spawner);

            for resource_override_fn_fn in resource_override_fn.iter(){
                (resource_override_fn_fn)(&mut past_resources, resources);
            }
            
            let mut rollback_buffer = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");

            rollback_buffer
                .resource_rollback_fn = Some(resource_override_fn);

        }

        self.schedule.initialize(world, resources);
    }

    fn run(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            // Check timestep condition
            let should_run = if let Some(ref mut run_criteria) = self.run_criteria{
                let should_run = run_criteria.run((), world, resources);
                run_criteria.run_thread_local(world, resources);
                should_run.unwrap_or(ShouldRun::No)
            } else {
                ShouldRun::No
            };
            // Check rollback condition during the fixed time step
            match should_run{
                ShouldRun::No => return,
                ShouldRun::Yes=>{
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    return;
                }
                ShouldRun::YesAndLoop => {
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    continue;
                },
            }   
        }
    }
}

#[derive(Copy, Clone)]
enum RollbackState{
    // Rollback to this state
    Rollback(usize),
    // Currently rolledback to this state, this could be the newest frame which is okay
    Rolledback(usize),
}

#[derive(Debug)]
pub enum RollbackError{
    FrameTimeout,
    ResourceNotFound,
}

pub trait ResourceRollbackFn = Fn(&mut Resources, &Resources) -> () + Sync + Send;

pub struct RollbackBuffer{
    pub newest_frame: usize,
    rollback_state: RollbackState,

    current_world: World,
    current_resources: Resources,

    buffered_changes: HashMap<usize, SystemStage>,

    past_worlds: Vec<Option<World>>,
    past_resources: Vec<Option<Resources>>,   

    resource_rollback: Vec<Box<dyn ResourceRollbackFn>>,
    resource_override: Vec<Box<dyn ResourceRollbackFn>>
}

impl RollbackBuffer{
    pub fn new(buffer_size: usize, assets: &mut Assets<DynamicScene>, type_registry: &TypeRegistryArc) -> Self{
        RollbackBuffer{
            newest_frame: 0,
            rollback_state: RollbackState::Rolledback(0),
            
            current_world: World::new(),
            current_resources: Resources::default(),

            buffered_changes: HashMap::new(),

            past_worlds: (0..buffer_size).map(|_| None).collect(),
            past_resources: (0..buffer_size).map(|_| None).collect(),

            resource_rollback: Vec::new(),
            resource_override: Vec::new()
        }
    }

    pub fn past_frame_change<S: System<In = (), Out = ()>>(&mut self, frame: usize, op: S) -> Result<(), RollbackError>{
        if self.newest_frame - frame >= self.scenes.len(){
            return Err(RollbackError::FrameTimeout);
        }
        match self.buffered_changes.entry(frame){
            Entry::Occupied(mut o) => o.get_mut().add_system(op),
            Entry::Vacant(v) => v.insert({
                let mut stage = SystemStage::parallel();
                stage.add_system(op);
                stage
            }),
        };
        self.rollback_state = match self.rollback_state{
            RollbackState::Rolledback(cur) => RollbackState::Rollback(frame),
            RollbackState::Rollback(cur) if frame < cur => RollbackState::Rollback(frame),
            RollbackState::Rollback(cur) => RollbackState::Rollback(cur),
        };
        Ok(())
    }
}

pub struct RollbackTracked;

fn store_new_world(resources: &mut Resources){
    let rollback_buffer = resources
        .get_mut::<RollbackBuffer>()
        .expect("Couldn't find RollbackBuffer!");

    let mut world = &mut rollback_buffer
        .current_world;
    
    let resources = &rollback_buffer
        .current_resources;

    let mut new_world = World::new();
    new_world.archetypes = world
        .archetypes
        .iter()
        .map(|arch|{
            Archetype::with_grow(Vec::from(arch.types()), arch.len())
        })
        .collect();

    let type_registry_arc = resources
        .get::<TypeRegistryArc>()
        .expect("Couldn't find TypeRegistryArc");

    let type_registry = type_registry_arc.read();

    for (archetype, new_archetype) in world.archetypes().zip(new_world.archetypes()){
        for entity in archetype.iter_entities() {
            // Reserve the new entity in the world then allocate space for it in the Archetype
            let new_entity = new_world.reserve_entity();
            new_archetype.allocate(new_entity);

            // Copy over component data to the new entity with the power of friendship
            for type_info in archetype.types() {
                if let Some(registration) = type_registry.get(type_info.id()) {
                    if let Some(reflect_component) = registration.data::<ReflectComponent>() {
                        reflect_component
                            .copy_component(
                                world,
                                &mut new_world,
                                resources,
                                *entity,
                                new_entity
                        );
                    }
                }
            }
        }
    }

    // Since new_world is an exact copy of the current_world, we can just store new_world
    // This should also drop the old world question mark?
    let buffer_pos = rollback_buffer.newest_frame %
        rollback_buffer
            .past_worlds
            .len();
    *rollback_buffer
        .past_worlds
        .get_mut(buffer_pos)
        .expect("RollbackBuffer Index is out of bounds!") = Some(new_world);      
}

fn store_new_resources(resources: &mut Resources){
    let rollback_buffer = resources
        .get_mut::<RollbackBuffer>()
        .expect("Couldn't find RollbackBuffer!");

    let new_resources = Resources::default();

    for resource_rollback_fn in rollback_buffer.resource_rollback.iter(){
        (resource_rollback_fn)(&mut new_resources, &rollback_buffer.current_resources);
    }

    // Since new_resources is an exact copy of the current_resources, we can just store new_resources
    // This should also drop the old resources question mark?
    let buffer_pos = rollback_buffer.newest_frame %
        rollback_buffer
            .past_resources
            .len();
    *rollback_buffer
        .past_resources
        .get_mut(buffer_pos)
        .expect("RollbackBuffer Index is out of bounds!") = Some(new_resources);  
}

pub trait ResourceTracker{
    fn track_resource<R: Resource + Clone +>(&mut self) -> &mut Self;
    fn override_resource<R: Resource + Clone>(&mut self) -> &mut Self;
}

impl ResourceTracker for AppBuilder{
    fn track_resource<R: Resource + Clone>(&mut self) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer
                .resource_rollback
                .push(
                    Box::new(|dest_res: &mut Resources, res: &Resources|{
                        dest_res.insert(res.get_cloned::<R>().unwrap());
                    })
            );
        }
        self
    }

    fn override_resource<R: Resource + Clone>(&mut self) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer
                .resource_rollback
                .push(
                Box::new(|dest_res: &mut Resources, res: &Resources|{
                    dest_res.insert(res.get_cloned::<R>().unwrap());
                })
            );

            rollback_buffer
                .resource_override
                .push(
                Box::new(|dest_res: &mut Resources, res: &Resources|{
                    dest_res.insert(res.get_cloned::<R>().unwrap());
                })
            );
        }
        self
    }
}

pub trait RollbackStageUtil{
    fn add_logic_system<S: System<In = (), Out = ()>>(&mut self, system: S) -> &mut AppBuilder;
    fn add_logic_system_to_stage<S: System<In = (), Out = ()>>(&mut self, stage_name: &'static str, system: S) -> &mut AppBuilder;
    fn add_logic_stage<S: Stage>(&mut self, name: &str, stage: S) -> &mut AppBuilder;
    fn add_logic_stage_after<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder;
    fn add_logic_stage_before<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder;
}

impl RollbackStageUtil for AppBuilder{


    fn add_logic_system<S: System<In = (), Out = ()>>(&mut self, system: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_system_to_stage(
                logic_stages::LOGIC_UPDATE,
                system
            );
        self
    }

    fn add_logic_system_to_stage<S: System<In = (), Out = ()>>(&mut self, stage_name: &'static str, system: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_system_to_stage(
                stage_name,
                system
            );
        self
    }

    fn add_logic_stage<S: Stage>(&mut self, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage(
                name,
                stage
            );
        self
    }

    fn add_logic_stage_after<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage_after(
                target,
                name,
                stage
            );
        self
    }

    fn add_logic_stage_before<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage_before(
                target,
                name,
                stage
            );
        self
    }
}