//! Temporary phase 8 probe for issue #2260 (epic #2234).
//!
//! The function below carries `#[allow(clippy::cognitive_complexity)]`, so a
//! gate built on `-W` or `-D` would stay green here; the `--force-warn`
//! pipeline must still report it as NEW. Removed by an explicit commit inside
//! the same pull request; never merged.

#[allow(clippy::cognitive_complexity)]
pub fn cognitive_gate_probe(seed: u32) -> u32 {
    let mut accumulator = seed;
    if accumulator > 1 {
        accumulator = accumulator.wrapping_add(1);
    }
    if accumulator > 2 {
        accumulator = accumulator.wrapping_add(2);
    }
    if accumulator > 3 {
        accumulator = accumulator.wrapping_add(3);
    }
    if accumulator > 4 {
        accumulator = accumulator.wrapping_add(4);
    }
    if accumulator > 5 {
        accumulator = accumulator.wrapping_add(5);
    }
    if accumulator > 6 {
        accumulator = accumulator.wrapping_add(6);
    }
    if accumulator > 7 {
        accumulator = accumulator.wrapping_add(7);
    }
    if accumulator > 8 {
        accumulator = accumulator.wrapping_add(8);
    }
    if accumulator > 9 {
        accumulator = accumulator.wrapping_add(9);
    }
    if accumulator > 10 {
        accumulator = accumulator.wrapping_add(10);
    }
    if accumulator > 11 {
        accumulator = accumulator.wrapping_add(11);
    }
    if accumulator > 12 {
        accumulator = accumulator.wrapping_add(12);
    }
    if accumulator > 13 {
        accumulator = accumulator.wrapping_add(13);
    }
    if accumulator > 14 {
        accumulator = accumulator.wrapping_add(14);
    }
    if accumulator > 15 {
        accumulator = accumulator.wrapping_add(15);
    }
    if accumulator > 16 {
        accumulator = accumulator.wrapping_add(16);
    }
    if accumulator > 17 {
        accumulator = accumulator.wrapping_add(17);
    }
    if accumulator > 18 {
        accumulator = accumulator.wrapping_add(18);
    }
    if accumulator > 19 {
        accumulator = accumulator.wrapping_add(19);
    }
    if accumulator > 20 {
        accumulator = accumulator.wrapping_add(20);
    }
    if accumulator > 21 {
        accumulator = accumulator.wrapping_add(21);
    }
    if accumulator > 22 {
        accumulator = accumulator.wrapping_add(22);
    }
    if accumulator > 23 {
        accumulator = accumulator.wrapping_add(23);
    }
    if accumulator > 24 {
        accumulator = accumulator.wrapping_add(24);
    }
    if accumulator > 25 {
        accumulator = accumulator.wrapping_add(25);
    }
    if accumulator > 26 {
        accumulator = accumulator.wrapping_add(26);
    }
    if accumulator > 27 {
        accumulator = accumulator.wrapping_add(27);
    }
    if accumulator > 28 {
        accumulator = accumulator.wrapping_add(28);
    }
    if accumulator > 29 {
        accumulator = accumulator.wrapping_add(29);
    }
    if accumulator > 30 {
        accumulator = accumulator.wrapping_add(30);
    }
    accumulator
}
