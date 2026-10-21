//! Dice: roll definitions (`NdM+B`), the random source, and d20 rolls with advantage.
//!
//! The engine's dice come from a 64-bit Mersenne Twister per *roll stream*. Each draw
//! re-seeds an engine from the stream's seed and throws away as many outputs as the stream's
//! draw counter, then takes the next one and bumps the counter
//! (`eoc::RollManager::InternalRollDice` 0x101c6139c, `RNGEngine::GetNextInteger` 0x101c5efb0,
//! 4.1.1.7631656). That is the same as reading one MT19937-64 sequence in order, which is what
//! [`RollStream`] does. A die is `min + raw % (max - min + 1)`.
//!
//! Not reproduced (yet): the "bell curve" and success/fail "debt" (karmic dice) modes that
//! `RollManager` switches on for some stream types; see `docs/formats/stats-expressions.md`.

use std::fmt;

/// Die sizes the stats languages accept (`EDiceType` 0–6).
// bg3: eoc::ParseDiceTypeSet (0x101288fe8) accepts exactly these face counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiceType {
    D4,
    D6,
    D8,
    D10,
    D12,
    D20,
    D100,
}

impl DiceType {
    pub fn faces(self) -> u32 {
        match self {
            DiceType::D4 => 4,
            DiceType::D6 => 6,
            DiceType::D8 => 8,
            DiceType::D10 => 10,
            DiceType::D12 => 12,
            DiceType::D20 => 20,
            DiceType::D100 => 100,
        }
    }

    pub fn from_faces(n: u32) -> Option<Self> {
        Some(match n {
            4 => DiceType::D4,
            6 => DiceType::D6,
            8 => DiceType::D8,
            10 => DiceType::D10,
            12 => DiceType::D12,
            20 => DiceType::D20,
            100 => DiceType::D100,
            _ => return None,
        })
    }

    /// `DiceType.d20` style label used in conditions.
    pub fn from_label(s: &str) -> Option<Self> {
        s.strip_prefix('d')
            .and_then(|n| n.parse().ok())
            .and_then(Self::from_faces)
    }
}

/// `count` dice of `dice` plus a flat `bonus` (`eoc::RollDefinition`). A plain integer is a
/// definition with no dice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RollDefinition {
    pub dice: Option<DiceType>,
    pub count: u8,
    pub bonus: i32,
}

impl RollDefinition {
    pub fn flat(bonus: i32) -> Self {
        Self {
            dice: None,
            count: 0,
            bonus,
        }
    }

    pub fn dice(count: u8, dice: DiceType, bonus: i32) -> Self {
        Self {
            dice: Some(dice),
            count,
            bonus,
        }
    }

    /// Parse `NdM`, `dM`, `NdM+B`, `NdM-B` or a non-negative integer.
    // bg3: eoc::ParseRoll (0x101c53668, 4.1.1.7631656): splits at the first `+` (else the first
    // `-`), dice part via ParseDiceTypeSet, the rest must be an integer; a `-` before the `d`
    // is rejected.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            return s.parse().ok().map(Self::flat);
        }
        let d = s.find('d')?;
        let (split, neg) = match s.find('+') {
            Some(p) => (Some(p), false),
            None => match s.find('-') {
                Some(p) if p < d => return None,
                Some(p) => (Some(p), true),
                None => (None, false),
            },
        };
        let (dice_part, bonus) = match split {
            Some(p) => {
                let b: i32 = s[p + 1..].trim().parse().ok()?;
                (&s[..p], if neg { -b } else { b })
            }
            None => (s, 0),
        };
        let dice_part = dice_part.trim();
        let (n, m) = dice_part.split_once('d')?;
        let count: u8 = if n.is_empty() { 1 } else { n.parse().ok()? };
        let faces: u32 = m.parse().ok()?;
        Some(Self::dice(count, DiceType::from_faces(faces)?, bonus))
    }

    /// Expected value as the engine computes it for tooltips/averages:
    /// `count * (faces + 1) / 2` (integer division) plus the bonus.
    // bg3: eoc::stats_expression::object_roll::Resolve (0x101f837d0), average mode
    pub fn average(&self) -> i32 {
        let dice = match self.dice {
            Some(d) => (self.count as i32 * (d.faces() as i32 + 1)) / 2,
            None => 0,
        };
        dice + self.bonus
    }

    pub fn max(&self) -> i32 {
        self.dice
            .map_or(0, |d| self.count as i32 * d.faces() as i32)
            + self.bonus
    }

    pub fn min(&self) -> i32 {
        self.count as i32 * self.dice.map_or(0, |_| 1) + self.bonus
    }

    /// Roll all dice and add the bonus.
    pub fn roll(&self, rng: &mut dyn DiceRng) -> i32 {
        let mut total = self.bonus;
        if let Some(d) = self.dice {
            for _ in 0..self.count {
                total += rng.roll_die(d.faces()) as i32;
            }
        }
        total
    }

    /// The same roll with twice the dice (critical hits double dice, not the bonus).
    // bg3: eoc::stats::CalculateDamage (0x101fd7408) doubles each roll's dice count on a crit
    pub fn doubled(&self) -> Self {
        Self {
            count: self.count.saturating_mul(2),
            ..*self
        }
    }
}

impl fmt::Display for RollDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dice {
            None => write!(f, "{}", self.bonus),
            Some(d) => {
                write!(f, "{}d{}", self.count, d.faces())?;
                match self.bonus {
                    0 => Ok(()),
                    b if b > 0 => write!(f, "+{b}"),
                    b => write!(f, "{b}"),
                }
            }
        }
    }
}

/// A source of die rolls. Inject a seeded [`RollStream`] for deterministic results.
pub trait DiceRng {
    /// A uniformly distributed integer in `1..=faces`.
    fn roll_die(&mut self, faces: u32) -> u32;
}

/// MT19937-64 (Matsumoto & Nishimura), the generator behind the engine's roll streams.
#[derive(Clone)]
pub struct Mt64 {
    mt: [u64; 312],
    idx: usize,
}

impl Mt64 {
    const NN: usize = 312;
    const MM: usize = 156;
    const MATRIX_A: u64 = 0xB502_6F5A_A966_19E9;
    const UM: u64 = 0xFFFF_FFFF_8000_0000;
    const LM: u64 = 0x7FFF_FFFF;

    pub fn new(seed: u64) -> Self {
        let mut mt = [0u64; 312];
        mt[0] = seed;
        for i in 1..Self::NN {
            mt[i] = 6364136223846793005u64
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Self { mt, idx: Self::NN }
    }

    pub fn next_u64(&mut self) -> u64 {
        if self.idx >= Self::NN {
            for i in 0..Self::NN {
                let x = (self.mt[i] & Self::UM) | (self.mt[(i + 1) % Self::NN] & Self::LM);
                let mut xa = x >> 1;
                if x & 1 != 0 {
                    xa ^= Self::MATRIX_A;
                }
                self.mt[i] = self.mt[(i + Self::MM) % Self::NN] ^ xa;
            }
            self.idx = 0;
        }
        let mut x = self.mt[self.idx];
        self.idx += 1;
        x ^= (x >> 29) & 0x5555_5555_5555_5555;
        x ^= (x << 17) & 0x71D6_7FFF_EDA6_0000;
        x ^= (x << 37) & 0xFFF7_EEE0_0000_0000;
        x ^= x >> 43;
        x
    }
}

/// A roll stream: seed plus draw counter, read as one MT19937-64 sequence.
#[derive(Clone)]
pub struct RollStream {
    seed: u64,
    counter: u64,
    engine: Mt64,
}

impl RollStream {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: 0,
            engine: Mt64::new(seed),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Number of values drawn so far (the engine's stream counter).
    pub fn counter(&self) -> u64 {
        self.counter
    }

    /// A uniform integer in `min..=max`.
    pub fn next_in(&mut self, min: i64, max: i64) -> i64 {
        self.counter += 1;
        let raw = self.engine.next_u64();
        let range = (max - min + 1) as u64;
        min + (raw % range) as i64
    }
}

impl DiceRng for RollStream {
    fn roll_die(&mut self, faces: u32) -> u32 {
        self.next_in(1, faces as i64) as u32
    }
}

/// Fixed rolls, for tests: returns the given values in order (then repeats the last).
pub struct ScriptedDice {
    values: Vec<u32>,
    pos: usize,
}

impl ScriptedDice {
    pub fn new(values: &[u32]) -> Self {
        Self {
            values: values.to_vec(),
            pos: 0,
        }
    }
}

impl DiceRng for ScriptedDice {
    fn roll_die(&mut self, faces: u32) -> u32 {
        let v = self
            .values
            .get(self.pos)
            .or(self.values.last())
            .copied()
            .unwrap_or(1);
        self.pos += 1;
        v.clamp(1, faces)
    }
}

/// One d20 as rolled for an attack, save or check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D20 {
    /// The die that counts.
    pub natural: u32,
    /// The other die of the pair, when advantage or disadvantage applied.
    pub discarded: Option<u32>,
}

/// Roll a d20 with advantage/disadvantage.
///
/// The engine always rolls two d20s per d20 for these rolls and keeps the higher (advantage)
/// or lower (disadvantage); advantage and disadvantage together cancel. Which die counts on a
/// straight roll is not confirmed; we keep the first.
// bg3: eoc::RollManager::InternalRollDiceWithBoosts (0x101c62184, 4.1.1.7631656)
pub fn roll_d20(rng: &mut dyn DiceRng, adv: crate::context::Advantage) -> D20 {
    let a = rng.roll_die(20);
    let b = rng.roll_die(20);
    match (adv.advantage, adv.disadvantage) {
        (true, false) => D20 {
            natural: a.max(b),
            discarded: Some(a.min(b)),
        },
        (false, true) => D20 {
            natural: a.min(b),
            discarded: Some(a.max(b)),
        },
        _ => D20 {
            natural: a,
            discarded: None,
        },
    }
}
