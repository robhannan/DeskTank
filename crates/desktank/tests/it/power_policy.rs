//! Acceptance tests for O3 power policy and pacing integration (SPEC §6.2, §9).

use std::time::Duration;

use desktank_platform::{PauseReason, PowerDecision, PowerPolicy, PowerSignals, PowerSource};
use desktank_render::{Tank, TankConfig};
use desktank_sim::{MaterialTable, World, WorldConfig};

fn fish_tank(seed: u64) -> Tank {
    let config = WorldConfig {
        width: 160,
        height: 90,
        seed,
        ..WorldConfig::default()
    };
    let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
    let tetra = world
        .species()
        .id("neon_tetra")
        .expect("neon_tetra built in");
    world.add_creature(tetra, 80, 45).unwrap();
    Tank::new(world, TankConfig::default())
}

#[test]
fn battery_drops_fps_to_15_without_changing_sim_ticks() {
    let policy = PowerPolicy::default();
    let ac_signals = PowerSignals {
        source: PowerSource::Ac,
        ..PowerSignals::default()
    };
    let bat_signals = PowerSignals {
        source: PowerSource::Battery { percent: Some(75) },
        ..PowerSignals::default()
    };

    let mut ac_tank = fish_tank(42);
    let mut bat_tank = fish_tank(42);

    let PowerDecision::Run { max_fps: ac_fps } = policy.evaluate(ac_signals, false) else {
        panic!("AC should run");
    };
    let PowerDecision::Run { max_fps: bat_fps } = policy.evaluate(bat_signals, false) else {
        panic!("75% battery should run");
    };
    assert_eq!(ac_fps, 30);
    assert_eq!(bat_fps, 15);
    ac_tank.set_max_fps(ac_fps);
    bat_tank.set_max_fps(bat_fps);

    let end = Duration::from_secs(60);
    let drive = |tank: &mut Tank| -> u32 {
        let mut now = Duration::ZERO;
        let mut frames = 0u32;
        while now <= end {
            let req = tank.tick(now);
            if req.redraw {
                frames += 1;
            }
            if now == end {
                break;
            }
            now = req.wake_at.min(end);
        }
        frames
    };

    let ac_frames = drive(&mut ac_tank);
    let bat_frames = drive(&mut bat_tank);

    assert_eq!(ac_tank.world().tick_count(), 1200);
    assert_eq!(bat_tank.world().tick_count(), 1200);
    assert_eq!(
        ac_tank.world().state_hash(),
        bat_tank.world().state_hash(),
        "power state must never change simulation state"
    );
    // 20 Hz moving fish wants 1 frame at t=0 plus 20/s over 60 s.
    assert_eq!(ac_frames, 1201);
    // 15 fps cap draws 1 frame at t=0 plus 15/s over 60 s = 901 frames.
    assert!(
        (890..=901).contains(&bat_frames),
        "expected ~901 frames at 15 fps over 60 s, got {bat_frames}"
    );
    assert!(bat_frames < ac_frames);
}

#[test]
fn switching_between_ac_and_battery_preserves_determinism() {
    let policy = PowerPolicy::default();
    let mut steady_ac = fish_tank(99);
    let mut switching = fish_tank(99);

    let run_segment =
        |tank: &mut Tank, start: Duration, stop: Duration, signals: PowerSignals| -> u32 {
            let PowerDecision::Run { max_fps } = policy.evaluate(signals, false) else {
                panic!("expected Run");
            };
            tank.set_max_fps(max_fps);
            let mut now = start;
            let mut frames = 0;
            while now <= stop {
                let req = tank.tick(now);
                if req.redraw {
                    frames += 1;
                }
                if now == stop {
                    break;
                }
                now = req.wake_at.min(stop);
            }
            frames
        };

    let ac = PowerSignals::default();
    let battery = PowerSignals {
        source: PowerSource::Battery { percent: Some(50) },
        ..PowerSignals::default()
    };

    let _ = run_segment(&mut steady_ac, Duration::ZERO, Duration::from_secs(60), ac);
    let f1 = run_segment(&mut switching, Duration::ZERO, Duration::from_secs(20), ac);
    let f2 = run_segment(
        &mut switching,
        Duration::from_secs(20),
        Duration::from_secs(40),
        battery,
    );
    let f3 = run_segment(
        &mut switching,
        Duration::from_secs(40),
        Duration::from_secs(60),
        ac,
    );

    assert_eq!(switching.world().tick_count(), 1200);
    assert_eq!(
        switching.world().state_hash(),
        steady_ac.world().state_hash()
    );
    assert!(
        f1 > 380 && f2 <= 302 && f3 > 380,
        "frames: {f1}, {f2}, {f3}"
    );
}

#[test]
fn low_power_lock_display_sleep_and_low_battery_pause_world_time() {
    let policy = PowerPolicy::default();
    let mut tank = fish_tank(7);
    let mut clock = Duration::ZERO;

    // Run for 1 simulated second (20 ticks).
    while clock <= Duration::from_secs(1) {
        let req = tank.tick(clock);
        if clock == Duration::from_secs(1) {
            break;
        }
        clock = req.wake_at.min(Duration::from_secs(1));
    }
    assert_eq!(tank.world().tick_count(), 20);
    let hash_before_pause = tank.world().state_hash();

    let pause_cases = [
        (
            PowerSignals {
                low_power_mode: true,
                ..PowerSignals::default()
            },
            false,
            PauseReason::LowPowerMode,
        ),
        (
            PowerSignals {
                session_locked: true,
                ..PowerSignals::default()
            },
            false,
            PauseReason::SessionLocked,
        ),
        (
            PowerSignals {
                display_asleep: true,
                ..PowerSignals::default()
            },
            false,
            PauseReason::DisplayAsleep,
        ),
        (
            PowerSignals {
                source: PowerSource::Battery { percent: Some(19) },
                ..PowerSignals::default()
            },
            false,
            PauseReason::LowBattery(19),
        ),
        (PowerSignals::default(), true, PauseReason::Hidden),
    ];

    for (signals, hidden, expected_reason) in pause_cases {
        assert_eq!(
            policy.evaluate(signals, hidden),
            PowerDecision::Pause(expected_reason)
        );
        tank.pause();
        // One hour passes while paused — host does not call `Tank::tick`.
        clock += Duration::from_hours(1);
        assert_eq!(tank.world().tick_count(), 20);
        assert_eq!(tank.world().state_hash(), hash_before_pause);

        // Resuming re-anchors the pacer with no catch-up ticks.
        assert_eq!(
            policy.evaluate(PowerSignals::default(), false),
            PowerDecision::Run { max_fps: 30 }
        );
        let req = tank.tick(clock);
        assert_eq!(req.sim_ticks, 0);
        assert_eq!(tank.world().tick_count(), 20);
    }
}

#[test]
fn saver_host_runs_over_lock_and_pauses_on_display_sleep_or_low_power() {
    let saver_policy = PowerPolicy {
        allow_over_lock: true,
        ..PowerPolicy::default()
    };
    let mut tank = fish_tank(11);
    let locked = PowerSignals {
        session_locked: true,
        ..PowerSignals::default()
    };
    assert_eq!(
        saver_policy.evaluate(locked, false),
        PowerDecision::Run { max_fps: 30 }
    );

    let mut now = Duration::ZERO;
    let end = Duration::from_secs(2);
    while now <= end {
        let req = tank.tick(now);
        if now == end {
            break;
        }
        now = req.wake_at.min(end);
    }
    assert_eq!(tank.world().tick_count(), 40);

    let locked_sleep = PowerSignals {
        display_asleep: true,
        ..locked
    };
    assert_eq!(
        saver_policy.evaluate(locked_sleep, false),
        PowerDecision::Pause(PauseReason::DisplayAsleep)
    );
    let locked_lpm = PowerSignals {
        low_power_mode: true,
        ..locked
    };
    assert_eq!(
        saver_policy.evaluate(locked_lpm, false),
        PowerDecision::Pause(PauseReason::LowPowerMode)
    );
}
