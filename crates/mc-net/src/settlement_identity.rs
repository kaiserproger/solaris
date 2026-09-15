//! The identity a generated inhabitant is minted with.
//!
//! A village template authors no `UUID` for the entity it places, so the lane
//! that spawns one mints it: a deterministic function of the placement's claim —
//! the source pool element, the piece's world origin and the entity's index in
//! its template. Two lanes need that identity and they must agree exactly:
//!
//! - the spawn lane, which seeds the entity it creates ([`settlement_candidate`]), and
//! - the settlement site descriptor, which reports a generated village's
//!   inhabitants so an owner can adopt the villagers that already exist.
//!
//! The site descriptor therefore does not re-derive identities from where a
//! villager stands: the claim is the generator's own record of the placement and
//! this is the one mapping from it to an entity identity.
//!
//! [`settlement_candidate`]: crate::play::settlement_candidate

/// The entity identity one generated placement's `claim` mints.
///
/// Deterministic and claim-scoped: the same claim always names the same entity,
/// and two different placements never collide in practice (a 128-bit FNV-1a
/// pair over the claim string, with two different offset bases).
#[must_use]
pub(crate) fn settlement_entity_uuid(claim: &str) -> uuid::Uuid {
    fn hash(seed: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(seed, |value, byte| {
            (value ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    let high = hash(0xCBF2_9CE4_8422_2325, claim.as_bytes());
    let low = hash(0x8422_2325_CBF2_9CE4, claim.as_bytes());
    uuid::Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_mints_one_stable_claim_scoped_identity() {
        let claim = "owner:villager@72,8";
        assert_eq!(settlement_entity_uuid(claim), settlement_entity_uuid(claim));
        assert_ne!(
            settlement_entity_uuid(claim),
            settlement_entity_uuid("owner:villager@616,8")
        );
        // The identity is a canonical hyphenated UUID, which is what the site
        // descriptor publishes and what `claim_resident` takes back.
        assert_eq!(settlement_entity_uuid(claim).to_string().len(), 36);
    }
}
