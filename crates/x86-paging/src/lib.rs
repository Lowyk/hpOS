#![no_std]

const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const USER: u64 = 4;
const LARGE: u64 = 1 << 7;
const NX: u64 = 1 << 63;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
pub const PAGE: u64 = 4096;
pub const HUGE_PAGE: u64 = 2 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    OutOfMemory,
    InvalidAddress,
    AlreadyMapped,
    HugePageConflict,
    CorruptTable,
    WritableCode,
    SupervisorConflict,
}

#[derive(Clone, Copy)]
pub struct Permissions {
    pub writable: bool,
    pub executable: bool,
    pub uncached: bool,
}
impl Permissions {
    pub const DATA: Self = Self {
        writable: true,
        executable: false,
        uncached: false,
    };
    pub const CODE: Self = Self {
        writable: false,
        executable: true,
        uncached: false,
    };
    pub const READ_ONLY: Self = Self {
        writable: false,
        executable: false,
        uncached: false,
    };
    fn flags(self, nx_enabled: bool) -> Result<u64, Error> {
        if self.writable && self.executable {
            return Err(Error::WritableCode);
        }
        Ok(PRESENT
            | if self.writable { WRITE } else { 0 }
            | if !self.executable && nx_enabled {
                NX
            } else {
                0
            }
            | if self.uncached { 0x18 } else { 0 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub physical: u64,
    pub writable: bool,
    pub executable: bool,
    pub page_size: u64,
    pub user_accessible: bool,
}

/// A store returns zeroed 4 KiB tables, and checks that accesses refer to its tables.
/// Its physical backing must remain allocated for the lifetime of the address space.
pub trait TableStore {
    fn allocate_table(&mut self) -> Result<u64, Error>;
    fn read(&self, table: u64, index: usize) -> Result<u64, Error>;
    fn write(&mut self, table: u64, index: usize, value: u64) -> Result<(), Error>;
}

pub struct Mapper<S: TableStore> {
    store: S,
    root: u64,
    nx_enabled: bool,
}
impl<S: TableStore> Mapper<S> {
    pub fn new(mut store: S, nx_enabled: bool) -> Result<Self, Error> {
        let root = store.allocate_table()?;
        check_physical(root, PAGE)?;
        Ok(Self {
            store,
            root,
            nx_enabled,
        })
    }
    pub fn root(&self) -> u64 {
        self.root
    }
    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    fn descend(&mut self, table: u64, index: usize, user: bool) -> Result<u64, Error> {
        let entry = self.store.read(table, index)?;
        if entry & PRESENT != 0 {
            if entry & LARGE != 0 {
                return Err(Error::HugePageConflict);
            }
            if user && entry & USER == 0 {
                return Err(Error::SupervisorConflict);
            }
            return Ok(entry & ADDRESS);
        }
        let child = self.store.allocate_table()?;
        check_physical(child, PAGE)?;
        self.store.write(
            table,
            index,
            child | PRESENT | WRITE | if user { USER } else { 0 },
        )?;
        Ok(child)
    }

    /// Mapping operations are supervisor-only. The caller manages TLB invalidation.
    pub fn map_4k(
        &mut self,
        virtual_address: u64,
        physical: u64,
        permissions: Permissions,
    ) -> Result<(), Error> {
        self.map(virtual_address, physical, permissions, false, false)
    }
    pub fn map_2m(
        &mut self,
        virtual_address: u64,
        physical: u64,
        permissions: Permissions,
    ) -> Result<(), Error> {
        self.map(virtual_address, physical, permissions, true, false)
    }
    pub fn map_user_4k(
        &mut self,
        address: u64,
        physical: u64,
        permissions: Permissions,
    ) -> Result<(), Error> {
        self.map(address, physical, permissions, false, true)
    }
    pub fn root_entry(&self, slot: usize) -> Result<u64, Error> {
        self.store.read(self.root, slot)
    }
    /// Import only a supervisor PML4 entry, without exposing it to userspace.
    pub fn install_supervisor_entry(&mut self, slot: usize, entry: u64) -> Result<(), Error> {
        if slot >= 512 || entry & PRESENT == 0 || entry & (USER | LARGE) != 0 {
            return Err(Error::SupervisorConflict);
        }
        if self.store.read(self.root, slot)? != 0 {
            return Err(Error::AlreadyMapped);
        }
        self.store.write(self.root, slot, entry)
    }
    fn map(
        &mut self,
        virtual_address: u64,
        physical: u64,
        permissions: Permissions,
        large: bool,
        user: bool,
    ) -> Result<(), Error> {
        let size = if large { HUGE_PAGE } else { PAGE };
        check_virtual(virtual_address, size)?;
        check_physical(physical, size)?;
        let flags = permissions.flags(self.nx_enabled)?;
        let indices = indices(virtual_address);
        let mut table = self.root;
        for &index in &indices[..if large { 2 } else { 3 }] {
            table = self.descend(table, index, user)?;
        }
        let index = indices[if large { 2 } else { 3 }];
        if self.store.read(table, index)? & PRESENT != 0 {
            return Err(Error::AlreadyMapped);
        }
        self.store.write(
            table,
            index,
            physical | flags | if large { LARGE } else { 0 } | if user { USER } else { 0 },
        )
    }

    pub fn translate(&self, address: u64) -> Result<Option<Translation>, Error> {
        check_virtual(address, 1)?;
        let mut table = self.root;
        let mut writable = true;
        let mut executable = true;
        let mut user_accessible = true;
        for (level, index) in indices(address).into_iter().enumerate() {
            let entry = self.store.read(table, index)?;
            if entry & PRESENT == 0 {
                return Ok(None);
            }
            writable &= entry & WRITE != 0;
            executable &= entry & NX == 0;
            user_accessible &= entry & USER != 0;
            if level == 3 || (level == 2 && entry & LARGE != 0) {
                let size = if level == 3 { PAGE } else { HUGE_PAGE };
                return Ok(Some(Translation {
                    physical: (entry & ADDRESS & !(size - 1)) | (address & (size - 1)),
                    writable,
                    executable,
                    page_size: size,
                    user_accessible,
                }));
            }
            if entry & LARGE != 0 {
                return Err(Error::CorruptTable);
            }
            table = entry & ADDRESS;
        }
        Err(Error::CorruptTable)
    }

    /// Removes a leaf only; empty intermediate tables remain owned by the address space.
    pub fn unmap_4k(&mut self, address: u64) -> Result<Option<u64>, Error> {
        check_virtual(address, PAGE)?;
        let indices = indices(address);
        let mut table = self.root;
        for &index in &indices[..3] {
            let entry = self.store.read(table, index)?;
            if entry & PRESENT == 0 {
                return Ok(None);
            }
            if entry & LARGE != 0 {
                return Err(Error::HugePageConflict);
            }
            table = entry & ADDRESS;
        }
        let old = self.store.read(table, indices[3])?;
        if old & PRESENT == 0 {
            return Ok(None);
        }
        self.store.write(table, indices[3], 0)?;
        Ok(Some(old & ADDRESS))
    }
}

fn check_physical(address: u64, size: u64) -> Result<(), Error> {
    if address & (size - 1) != 0 || address >= 1u64 << 52 {
        Err(Error::InvalidAddress)
    } else {
        Ok(())
    }
}
fn check_virtual(address: u64, size: u64) -> Result<(), Error> {
    let upper = address >> 47;
    if address & (size - 1) != 0 || (upper != 0 && upper != 0x1ffff) {
        Err(Error::InvalidAddress)
    } else {
        Ok(())
    }
}
fn indices(address: u64) -> [usize; 4] {
    [39, 30, 21, 12].map(|shift| ((address >> shift) & 511) as usize)
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Store {
        tables: BTreeMap<u64, [u64; 512]>,
        budget: usize,
    }
    impl TableStore for Store {
        fn allocate_table(&mut self) -> Result<u64, Error> {
            if self.tables.len() == self.budget {
                return Err(Error::OutOfMemory);
            }
            let address = 0x200000 + self.tables.len() as u64 * PAGE;
            self.tables.insert(address, [0; 512]);
            Ok(address)
        }
        fn read(&self, table: u64, index: usize) -> Result<u64, Error> {
            self.tables
                .get(&table)
                .and_then(|t| t.get(index))
                .copied()
                .ok_or(Error::CorruptTable)
        }
        fn write(&mut self, table: u64, index: usize, value: u64) -> Result<(), Error> {
            *self
                .tables
                .get_mut(&table)
                .and_then(|t| t.get_mut(index))
                .ok_or(Error::CorruptTable)? = value;
            Ok(())
        }
    }
    fn mapper(nx: bool, budget: usize) -> Mapper<Store> {
        Mapper::new(
            Store {
                tables: BTreeMap::new(),
                budget,
            },
            nx,
        )
        .unwrap()
    }
    #[test]
    fn mixed_sizes_preserve_offsets_and_permissions() {
        let mut m = mapper(true, 10);
        m.map_4k(0x100000, 0x100000, Permissions::CODE).unwrap();
        m.map_2m(0x200000, 0x600000, Permissions::DATA).unwrap();
        assert_eq!(
            m.translate(0x100123).unwrap(),
            Some(Translation {
                physical: 0x100123,
                writable: false,
                executable: true,
                page_size: PAGE,
                user_accessible: false
            })
        );
        assert_eq!(
            m.translate(0x3fffff).unwrap(),
            Some(Translation {
                physical: 0x7fffff,
                writable: true,
                executable: false,
                page_size: HUGE_PAGE,
                user_accessible: false
            })
        );
        assert_eq!(m.translate(0).unwrap(), None);
        assert_eq!(m.translate(0x7f000).unwrap(), None);
    }
    #[test]
    fn invalid_mappings_never_replace_existing_code() {
        let mut m = mapper(true, 10);
        m.map_4k(0x100000, 0x100000, Permissions::CODE).unwrap();
        assert_eq!(
            m.map_4k(0x100000, 0x300000, Permissions::DATA),
            Err(Error::AlreadyMapped)
        );
        assert!(!m.translate(0x100000).unwrap().unwrap().writable);
        assert_eq!(
            m.map_4k(0x300001, 0x300000, Permissions::DATA),
            Err(Error::InvalidAddress)
        );
        assert_eq!(
            m.map_4k(0x0000_8000_0000_0000, 0x300000, Permissions::DATA),
            Err(Error::InvalidAddress)
        );
        assert_eq!(
            m.map_4k(
                0x300000,
                0x300000,
                Permissions {
                    writable: true,
                    executable: true,
                    uncached: false
                }
            ),
            Err(Error::WritableCode)
        );
    }
    #[test]
    fn scratch_mapping_can_be_removed_without_touching_identity_map() {
        let mut m = mapper(true, 10);
        m.map_2m(0x200000, 0x200000, Permissions::DATA).unwrap();
        m.map_4k(0x1_0000_1000, 0x300000, Permissions::DATA)
            .unwrap();
        assert_eq!(m.unmap_4k(0x1_0000_1000).unwrap(), Some(0x300000));
        assert_eq!(m.translate(0x1_0000_1000).unwrap(), None);
        assert!(m.translate(0x300000).unwrap().is_some());
        assert_eq!(m.unmap_4k(0x200000), Err(Error::HugePageConflict));
    }
    #[test]
    fn no_nx_cpu_never_gets_reserved_nx_bits() {
        let mut m = mapper(false, 10);
        m.map_4k(0x300000, 0x300000, Permissions::READ_ONLY)
            .unwrap();
        let t = m.translate(0x300000).unwrap().unwrap();
        assert!(!t.writable);
        assert!(t.executable);
    }
    #[test]
    fn allocation_failure_is_reported_with_target_still_unmapped() {
        let mut m = mapper(true, 2);
        assert_eq!(
            m.map_4k(0x300000, 0x300000, Permissions::DATA),
            Err(Error::OutOfMemory)
        );
        assert_eq!(m.translate(0x300000).unwrap(), None);
    }
    #[test]
    fn user_mapping_cannot_promote_existing_kernel_ancestors() {
        let mut m = mapper(false, 32);
        m.map_4k(0x100000, 0x100000, Permissions::CODE).unwrap();
        assert_eq!(
            m.map_user_4k(0x300000, 0x900000, Permissions::DATA),
            Err(Error::SupervisorConflict)
        );
        assert!(!m.translate(0x100000).unwrap().unwrap().user_accessible);
        let user = 1 << 39;
        m.map_user_4k(user, 0x900000, Permissions::CODE).unwrap();
        assert!(m.translate(user).unwrap().unwrap().user_accessible);
        assert!(!m.translate(user).unwrap().unwrap().writable);
        m.map_4k(user + PAGE, 0xa00000, Permissions::READ_ONLY)
            .unwrap();
        assert!(!m.translate(user + PAGE).unwrap().unwrap().user_accessible);
    }
    #[test]
    fn separate_roots_map_same_user_address_to_distinct_frames() {
        let mut first = mapper(false, 64);
        first.map_4k(0x100000, 0x100000, Permissions::CODE).unwrap();
        let kernel_entry = first.root_entry(0).unwrap();
        let user = 1 << 39;
        first
            .map_user_4k(user, 0x900000, Permissions::CODE)
            .unwrap();
        let store = Store {
            tables: first.store.tables.clone(),
            budget: 64,
        };
        let mut second = Mapper::new(store, false).unwrap();
        second.install_supervisor_entry(0, kernel_entry).unwrap();
        second
            .map_user_4k(user, 0xa00000, Permissions::CODE)
            .unwrap();
        assert_ne!(first.root(), second.root());
        assert_eq!(first.translate(user).unwrap().unwrap().physical, 0x900000);
        assert_eq!(second.translate(user).unwrap().unwrap().physical, 0xa00000);
        assert!(!second.translate(0x100000).unwrap().unwrap().user_accessible);
    }
    #[test]
    fn imported_kernel_entry_must_remain_supervisor_only() {
        let mut m = mapper(true, 4);
        assert_eq!(
            m.install_supervisor_entry(0, 0x200007),
            Err(Error::SupervisorConflict)
        );
        assert_eq!(m.root_entry(0).unwrap(), 0);
    }
    #[test]
    fn high_user_framebuffer_grant_does_not_expose_kernel_or_other_roots() {
        let user = 0xffff_ffff_8000_0000;
        let framebuffer = user + 0x1000000;
        let mut kernel = mapper(false, 64);
        kernel
            .map_4k(0x100000, 0x100000, Permissions::CODE)
            .unwrap();
        let entry = kernel.root_entry(0).unwrap();
        let mut input = Mapper::new(
            Store {
                tables: kernel.store.tables.clone(),
                budget: 64,
            },
            false,
        )
        .unwrap();
        input.install_supervisor_entry(0, entry).unwrap();
        input
            .map_user_4k(user, 0x900000, Permissions::CODE)
            .unwrap();
        let mut display = Mapper::new(
            Store {
                tables: input.store.tables.clone(),
                budget: 64,
            },
            false,
        )
        .unwrap();
        display.install_supervisor_entry(0, entry).unwrap();
        display
            .map_user_4k(user, 0xa00000, Permissions::CODE)
            .unwrap();
        let mut rights = Permissions::DATA;
        rights.uncached = true;
        display
            .map_user_4k(framebuffer, 0xe0000000, rights)
            .unwrap();
        assert_eq!(input.translate(framebuffer).unwrap(), None);
        assert_eq!(kernel.translate(framebuffer).unwrap(), None);
        let m = display.translate(framebuffer + 64).unwrap().unwrap();
        assert!(m.user_accessible && m.writable);
        assert_eq!(m.physical, 0xe0000040);
        assert!(
            !display
                .translate(0x100000)
                .unwrap()
                .unwrap()
                .user_accessible
        );
        assert_eq!(
            display.map_user_4k(0x200000, 0xb00000, Permissions::DATA),
            Err(Error::SupervisorConflict)
        );
    }
}
