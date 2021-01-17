use bevy::prelude::*;
use bevy::ecs::{SystemParam, ResourceIndex, FetchSystemParam, FetchRes, SystemState, TypeAccess};
use super::RollbackBuffer;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::marker::PhantomData;
use std::any::TypeId;

#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug)]
pub(crate) struct L<T>{
    phantom: PhantomData<T>,
}

impl<T> L<T>{
    pub fn new(data: &T) -> Self{
        Self{
            phantom: PhantomData
        }
    }
}

#[derive(Debug)]
pub struct LRes<'a, T:Resource>{
    value: &'a T,
}

impl<'a, T: Resource> LRes<'a, T>{
    pub unsafe fn new(value: NonNull<T>) -> Self{
        Self{
            value: &*value.as_ptr(),
        }
    }
}

impl<'a, T: Resource> Deref for LRes<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<'a, T: Resource>  SystemParam for LRes<'a, T>{
    type Fetch = FetchLRes<T>;
}

pub struct FetchLRes<T>(PhantomData<T>);

impl<'a, T: Resource> FetchSystemParam<'a> for FetchLRes<T>{
    type Item = LRes<'a, T>;

    fn init(system_state: &mut SystemState, _world: &World, resources: &mut Resources) {
        if system_state.resource_access.is_write(&TypeId::of::<L<T>>()){
            panic!(
                "System '{}' has a `LRes<{res}>` parameter that conflicts with \
                another parameter with mutable access to the same `{res}` resource.",
                system_state.name,
                res = std::any::type_name::<T>()
            );
        }
        if system_state.resource_access.is_write(&TypeId::of::<RollbackBuffer>()){
            panic!(
                "System '{}' is trying to access Logical Resources while mutating the RollbackBuffer!",
                system_state.name
            );
        }
        system_state.resource_access.add_read(TypeId::of::<RollbackBuffer>());
        system_state.resource_access.add_read(TypeId::of::<L<T>>());
    }

    #[inline]
    unsafe fn get_param(
        _system_state: &'a SystemState,
        _world: &'a World,
        resources: &'a Resources,
    ) -> Option<Self::Item> {
        let rollback_buffer = resources.get::<RollbackBuffer>().expect("Couldn't acquire RollbackBuffer!");
        Some(
            LRes::new(rollback_buffer.current_resources.get_unsafe_ref::<T>(ResourceIndex::Global)),
        )
    }
}


#[derive(Debug)]
pub struct LResMut<'a, T:Resource>{
    _marker: PhantomData<&'a T>,
    value: *mut T,
    mutated: *mut bool,
}

impl<'a, T: Resource> LResMut<'a, T>{
    pub unsafe fn new(value: NonNull<T>, mutated: NonNull<bool>) -> Self {
        Self {
            value: value.as_ptr(),
            mutated: mutated.as_ptr(),
            _marker: Default::default(),
        }
    }
}

impl<'a, T: Resource> Deref for LResMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe{ &*self.value }
    }
}

impl<'a, T: Resource> DerefMut for LResMut<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe{
            *self.mutated = true;
            &mut *self.value
        }
    }
}

impl<'a, T: Resource>  SystemParam for LResMut<'a, T>{
    type Fetch = FetchLResMut<T>;
}

pub struct FetchLResMut<T>(PhantomData<T>);

impl<'a, T: Resource> FetchSystemParam<'a> for FetchLResMut<T>{
    type Item = LResMut<'a, T>;

    fn init(system_state: &mut SystemState, _world: &World, resources: &mut Resources) {
        if system_state.resource_access.is_read_or_write(&TypeId::of::<T>()) {
            panic!(
                "System '{}' has a `LRes<{res}>` or `LResMut<{res}>` parameter that conflicts with \
                another parameter with mutable access to the same `{res}` resource.",
                system_state.name,
                res = std::any::type_name::<T>()
            );
        }
        if system_state.resource_access.is_write(&TypeId::of::<RollbackBuffer>()){
            panic!(
                "System '{}' is trying to access Logical Resources while mutating the RollbackBuffer!",
                system_state.name
            );
        }
        system_state.resource_access.add_read(TypeId::of::<RollbackBuffer>());
        system_state.resource_access.add_write(TypeId::of::<T>());
    }

    #[inline]
    unsafe fn get_param(
        _system_state: &'a SystemState,
        _world: &'a World,
        resources: &'a Resources,
    ) -> Option<Self::Item> {
        let rollback_buffer = resources.get::<RollbackBuffer>().expect("Couldn't acquire RollbackBuffer!");
        let (value, _added, mutated) = rollback_buffer.current_resources.get_unsafe_ref_with_added_and_mutated::<T>(ResourceIndex::Global);
        Some(
            LResMut::new(value, mutated),
        )
    }
}