use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, Ruleset, RulesetAttr,
    RulesetCreatedAttr, path_beneath_rules,
};
use tracing::debug;

use crate::error::Result;
use crate::policy::LandlockPlan;

pub const TCP_RULES_MIN_ABI: u32 = 4;

pub fn apply(plan: &LandlockPlan) -> Result<()> {
    if plan.is_empty() {
        debug!("skipping Landlock restrictions for this policy");
        return Ok(());
    }

    let abi = ABI::V6;
    let mut ruleset = Ruleset::default().set_compatibility(CompatLevel::BestEffort);

    if plan.has_fs_rules() {
        ruleset = ruleset.handle_access(AccessFs::from_all(abi))?;
    }
    if !plan.tcp_bind_ports.is_empty() {
        ruleset = ruleset.handle_access(AccessNet::BindTcp)?;
    }
    if !plan.tcp_connect_ports.is_empty() {
        ruleset = ruleset.handle_access(AccessNet::ConnectTcp)?;
    }

    let mut ruleset = ruleset.create()?.set_no_new_privs(false);

    if plan.has_fs_rules() {
        ruleset =
            ruleset.add_rules(path_beneath_rules(&plan.ro_paths, AccessFs::from_read(abi)))?;
        ruleset = ruleset.add_rules(path_beneath_rules(&plan.rw_paths, AccessFs::from_all(abi)))?;
    }

    for port in &plan.tcp_bind_ports {
        ruleset = ruleset.add_rule(NetPort::new(*port, AccessNet::BindTcp))?;
    }
    for port in &plan.tcp_connect_ports {
        ruleset = ruleset.add_rule(NetPort::new(*port, AccessNet::ConnectTcp))?;
    }

    let status = ruleset.restrict_self()?;
    debug!(?status, "applied Landlock restrictions");
    Ok(())
}

pub fn detect_abi() -> Option<u32> {
    const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;

    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };

    if version >= 0 {
        return u32::try_from(version).ok();
    }

    None
}
