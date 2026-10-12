// SPDX-License-Identifier: AGPL-3.0-or-later

//! The access-control chain (spec §8): replaying `ACLC` records into the
//! current members, roles, devices and epoch, and checking that content
//! records are signed by someone the chain authorizes (spec §7.3).

use std::collections::BTreeMap;

use imaginarium_format::{
    DocId, Limits, ObjectId, Record, RecordBuilder, RecordFlags, RecordType, pb,
};
use prost::Message;

use crate::error::{CryptoError, Result};
use crate::keys::KeyId;
use crate::rng::Randomness;
use crate::sign::{
    DeviceKey, DevicePublic, IdentityKey, IdentityPublic, Signer, Verifier, sign_record,
    signer_key_id, verify_record,
};

/// X-Wing public key length (spec §6.4.2).
pub const XWING_PUBLIC_KEY_LEN: usize = 1216;

/// A member's role (spec §8). Roles are ordered: each includes the
/// permissions of the roles below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Can read.
    Viewer,
    /// Can read and write the discussion stream.
    Commenter,
    /// Can read and write the document.
    Editor,
    /// Can also manage members below admin and rotate keys.
    Admin,
    /// Can do everything. Exactly one per document.
    Owner,
}

impl Role {
    /// Parses a protobuf `Role`. `ROLE_UNSPECIFIED` and unknown values fail.
    pub fn from_pb(value: i32) -> Result<Self> {
        match pb::Role::try_from(value) {
            Ok(pb::Role::Viewer) => Ok(Role::Viewer),
            Ok(pb::Role::Commenter) => Ok(Role::Commenter),
            Ok(pb::Role::Editor) => Ok(Role::Editor),
            Ok(pb::Role::Admin) => Ok(Role::Admin),
            Ok(pb::Role::Owner) => Ok(Role::Owner),
            Ok(pb::Role::Unspecified) | Err(_) => Err(acl(format!("invalid role {value}"))),
        }
    }

    /// The protobuf `Role`.
    pub fn to_pb(self) -> pb::Role {
        match self {
            Role::Viewer => pb::Role::Viewer,
            Role::Commenter => pb::Role::Commenter,
            Role::Editor => pb::Role::Editor,
            Role::Admin => pb::Role::Admin,
            Role::Owner => pb::Role::Owner,
        }
    }
}

/// A current member of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The member's signature keys.
    pub identity: IdentityPublic,
    /// The member's X-Wing key, for wrapping epoch keys to them.
    pub xwing_public_key: Vec<u8>,
    /// The member's role.
    pub role: Role,
}

/// A current device of a member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Key id of the member the device belongs to.
    pub member_id: KeyId,
    /// The device's signing key.
    pub public: DevicePublic,
    /// The device's X-Wing key, for wrapping epoch keys to it.
    pub xwing_public_key: Vec<u8>,
}

/// What a key was allowed to do in each epoch, kept for keys that are no
/// longer current, so that records written before a removal still verify.
#[derive(Clone, Debug)]
struct Grant<P> {
    public: P,
    /// The highest role the key held at any point while the epoch was current.
    roles: BTreeMap<u32, Role>,
}

impl<P> Grant<P> {
    fn note(&mut self, epoch: u32, role: Role) {
        let entry = self.roles.entry(epoch).or_insert(role);
        *entry = (*entry).max(role);
    }

    fn role_in(&self, epoch: u32) -> Option<Role> {
        self.roles.get(&epoch).copied()
    }
}

/// The state after replaying an ACL chain (spec §8).
///
/// Build it with [`AclState::replay`] (all of a document's `ACLC` records)
/// or [`AclState::genesis`], then extend it with [`AclState::apply`].
#[derive(Clone, Debug)]
pub struct AclState {
    doc_id: DocId,
    next_index: u64,
    head_hash: [u8; 32],
    genesis_owner: KeyId,
    owner: KeyId,
    epoch: u32,
    rotation_required: bool,
    members: BTreeMap<KeyId, Member>,
    devices: BTreeMap<KeyId, Device>,
    identity_grants: BTreeMap<KeyId, Grant<IdentityPublic>>,
    device_grants: BTreeMap<KeyId, Grant<DevicePublic>>,
}

impl AclState {
    /// Starts a chain from its `Genesis` record (spec §8.2).
    pub fn genesis(doc_id: &DocId, record: &Record) -> Result<Self> {
        let change = decode_change(doc_id, record)?;
        if change.index != 0 || change.prev_hash != [0u8; 32] {
            return Err(acl(
                "the genesis change must have index 0 and a zero prev_hash",
            ));
        }
        let Some(pb::acl_change::Change::Genesis(genesis)) = change.change else {
            return Err(acl("the first change must be a genesis"));
        };
        if genesis.epoch != 1 {
            return Err(acl("the genesis epoch must be 1"));
        }
        let owner = parse_identity(genesis.owner.as_ref())?;
        let owner_id = owner.identity.key_id();
        // The genesis is signed by the owner it creates (self-signed).
        verify_record(record, doc_id, &Verifier::Identity(owner.identity.clone()))?;
        let device = parse_device(genesis.device.as_ref(), owner_id)?;

        let mut state = Self {
            doc_id: *doc_id,
            next_index: 0,
            head_hash: [0; 32],
            genesis_owner: owner_id,
            owner: owner_id,
            epoch: 1,
            rotation_required: false,
            members: BTreeMap::new(),
            devices: BTreeMap::new(),
            identity_grants: BTreeMap::new(),
            device_grants: BTreeMap::new(),
        };
        state.members.insert(
            owner_id,
            Member {
                identity: owner.identity,
                xwing_public_key: owner.xwing,
                role: Role::Owner,
            },
        );
        state.devices.insert(device.public.key_id(), device);
        state.advance(record);
        Ok(state)
    }

    /// Replays all of a document's `ACLC` records, in any order (spec §8.2).
    ///
    /// Identical copies of a record are ignored. Two different changes with
    /// the same index are a fork, reported as [`CryptoError::AclFork`]. A
    /// missing index is an error.
    pub fn replay<'r>(
        doc_id: &DocId,
        records: impl IntoIterator<Item = &'r Record>,
    ) -> Result<Self> {
        let mut by_index: BTreeMap<u64, (&Record, pb::AclChange)> = BTreeMap::new();
        for record in records {
            if record.rtype() != RecordType::ACLC {
                continue;
            }
            let change = decode_change(doc_id, record)?;
            match by_index.get(&change.index) {
                Some((existing, _)) if existing.encode() != record.encode() => {
                    return Err(CryptoError::AclFork(change.index));
                }
                Some(_) => {}
                None => {
                    by_index.insert(change.index, (record, change));
                }
            }
        }
        let mut changes = by_index.into_values();
        let (first, _) = changes
            .next()
            .ok_or_else(|| acl("the document has no ACL chain"))?;
        let mut state = Self::genesis(doc_id, first)?;
        for (record, change) in changes {
            state.apply_decoded(record, change)?;
        }
        Ok(state)
    }

    /// Applies the next change of the chain, after checking its position,
    /// signature and permissions (spec §8.2–§8.3). A rejected change leaves
    /// the state unchanged.
    pub fn apply(&mut self, record: &Record) -> Result<()> {
        let change = decode_change(&self.doc_id, record)?;
        self.apply_decoded(record, change)
    }

    fn apply_decoded(&mut self, record: &Record, change: pb::AclChange) -> Result<()> {
        if change.index != self.next_index {
            return Err(acl(format!(
                "expected change {}, found {}",
                self.next_index, change.index
            )));
        }
        if change.prev_hash != self.head_hash {
            return Err(acl(format!(
                "change {} doesn't follow the current head",
                change.index
            )));
        }
        let signer_id = signer_key_id(record)?;
        let signer = self
            .members
            .get(&signer_id)
            .ok_or_else(|| acl("the change is signed by a non-member"))?;
        verify_record(
            record,
            &self.doc_id,
            &Verifier::Identity(signer.identity.clone()),
        )?;
        let signer_role = signer.role;

        // Every arm checks everything before it changes anything, so a
        // rejected change leaves the state as it was (no clone needed).
        let next = &mut *self;
        use pb::acl_change::Change;
        match change.change.ok_or_else(|| acl("empty change"))? {
            Change::Genesis(_) => return Err(acl("only the first change may be a genesis")),
            Change::AddMember(add) => {
                let member = parse_identity(add.member.as_ref())?;
                let member_id = member.identity.key_id();
                let role = Role::from_pb(add.role)?;
                if role == Role::Owner {
                    return Err(acl(
                        "there is exactly one owner; transfer ownership with SetRole",
                    ));
                }
                if next.members.contains_key(&member_id) {
                    return Err(acl("already a member"));
                }
                if !may_manage(signer_role, None, role) {
                    return Err(acl("not allowed to add a member with this role"));
                }
                next.members.insert(
                    member_id,
                    Member {
                        identity: member.identity,
                        xwing_public_key: member.xwing,
                        role,
                    },
                );
            }
            Change::SetRole(set) => {
                let member_id = key_id(&set.member_id)?;
                let role = Role::from_pb(set.role)?;
                let old = next.member(&member_id)?.role;
                if role == Role::Owner {
                    // Ownership transfer (spec §8.3): by the owner, who becomes admin.
                    if signer_role != Role::Owner {
                        return Err(acl("only the owner can transfer ownership"));
                    }
                    if member_id == next.owner {
                        return Err(acl("already the owner"));
                    }
                } else if old == Role::Owner {
                    return Err(acl(
                        "the owner's role changes only by transferring ownership",
                    ));
                } else if !may_manage(signer_role, Some(old), role) {
                    return Err(acl("not allowed to change this member's role"));
                }
                if role == Role::Owner {
                    let previous = std::mem::replace(&mut next.owner, member_id);
                    if let Some(member) = next.members.get_mut(&previous) {
                        member.role = Role::Admin;
                    }
                }
                if let Some(member) = next.members.get_mut(&member_id) {
                    member.role = role;
                }
            }
            Change::RemoveMember(remove) => {
                let member_id = key_id(&remove.member_id)?;
                let member = next.member(&member_id)?;
                if !may_remove(signer_role, member.role) {
                    return Err(acl("not allowed to remove this member"));
                }
                next.members.remove(&member_id);
                next.devices
                    .retain(|_, device| device.member_id != member_id);
                next.rotation_required = true;
            }
            Change::AddDevice(add) => {
                let member_id = key_id(&add.member_id)?;
                let member = next.member(&member_id)?;
                if signer_id != member_id && !may_remove(signer_role, member.role) {
                    return Err(acl("not allowed to add a device for this member"));
                }
                let device = parse_device(add.device.as_ref(), member_id)?;
                let device_id = device.public.key_id();
                if next.devices.contains_key(&device_id) {
                    return Err(acl("the device is already registered"));
                }
                next.devices.insert(device_id, device);
            }
            Change::RemoveDevice(remove) => {
                let member_id = key_id(&remove.member_id)?;
                let device_id = key_id(&remove.device_id)?;
                let member = next.member(&member_id)?;
                if signer_id != member_id && !may_remove(signer_role, member.role) {
                    return Err(acl("not allowed to remove a device of this member"));
                }
                match next.devices.get(&device_id) {
                    Some(device) if device.member_id == member_id => {}
                    _ => return Err(acl("the member has no such device")),
                }
                next.devices.remove(&device_id);
                // The chain can't tell which devices received keys, so every
                // removed device is treated as having held keys (spec §8.3).
                next.rotation_required = true;
            }
            Change::RotateEpoch(rotate) => {
                if signer_role < Role::Admin {
                    return Err(acl("only the owner and admins rotate keys"));
                }
                if Some(rotate.epoch) != next.epoch.checked_add(1) {
                    return Err(acl(format!(
                        "the next epoch is {}",
                        next.epoch.saturating_add(1)
                    )));
                }
                next.epoch = rotate.epoch;
                next.rotation_required = false;
            }
        }
        self.advance(record);
        Ok(())
    }

    /// Builds and signs the next change on top of the current head.
    pub fn next_change(
        &self,
        change: pb::acl_change::Change,
        signer: &IdentityKey,
        rng: &mut dyn Randomness,
    ) -> Result<Record> {
        change_record(
            &self.doc_id,
            self.next_index,
            self.head_hash,
            change,
            signer,
            rng,
        )
    }

    /// The document this chain belongs to.
    pub fn doc_id(&self) -> &DocId {
        &self.doc_id
    }

    /// The index the next change must have.
    pub fn next_index(&self) -> u64 {
        self.next_index
    }

    /// BLAKE3 of the last applied record: the next change's `prev_hash`.
    pub fn head_hash(&self) -> [u8; 32] {
        self.head_hash
    }

    /// The current owner's key id.
    pub fn owner(&self) -> KeyId {
        self.owner
    }

    /// The key id of the identity that signed the `Genesis`, which stays
    /// the same when ownership is transferred. Readers that unlock with a
    /// recipient key check it against the owner they expect (ADR-0018).
    pub fn genesis_owner(&self) -> KeyId {
        self.genesis_owner
    }

    /// The current key epoch.
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Whether a member or device was removed since the last rotation. If
    /// so, writers MUST append a `RotateEpoch` before new content (spec §8.3).
    pub fn rotation_required(&self) -> bool {
        self.rotation_required
    }

    /// The current members, by identity key id.
    pub fn members(&self) -> &BTreeMap<KeyId, Member> {
        &self.members
    }

    /// The current devices, by device key id.
    pub fn devices(&self) -> &BTreeMap<KeyId, Device> {
        &self.devices
    }

    /// The current role of a member.
    pub fn role(&self, member_id: &KeyId) -> Option<Role> {
        self.members.get(member_id).map(|member| member.role)
    }

    /// Whether a current device may write new records to `stream` now
    /// (spec §7.3). False while a rotation is pending.
    pub fn can_write(&self, device_id: &KeyId, stream: pb::Stream) -> bool {
        let Some(role) = self
            .devices
            .get(device_id)
            .and_then(|device| self.role(&device.member_id))
        else {
            return false;
        };
        !self.rotation_required && role >= required_role(RecordType::UPDT, stream)
    }

    /// Verifies a content record's signature against the chain (spec §7.3).
    ///
    /// The signer must be a device whose member held the required role at
    /// some point while the record's epoch was current, so records written
    /// before a removal stay valid. Records that need no signature (`BLOB`,
    /// `META`, …) are accepted as they are: they are protected by
    /// encryption alone.
    pub fn verify_content(&self, record: &Record) -> Result<()> {
        let rtype = record.rtype();
        if !matches!(
            rtype,
            RecordType::UPDT | RecordType::SNAP | RecordType::VERS
        ) {
            return Ok(());
        }
        let epoch = record.header().epoch;
        if epoch == 0 {
            return Err(acl("unencrypted content in a document with an ACL chain"));
        }
        let signer_id = signer_key_id(record)?;
        let grant = self
            .device_grants
            .get(&signer_id)
            .ok_or_else(|| acl("signed by a device the chain doesn't know"))?;
        verify_record(record, &self.doc_id, &Verifier::Device(grant.public))?;
        let required = required_role(rtype, record.header().stream());
        match grant.role_in(epoch) {
            Some(role) if role >= required => Ok(()),
            _ => Err(acl(format!(
                "the signer wasn't allowed to write this record in epoch {epoch}"
            ))),
        }
    }

    /// Verifies a `KEYR` record's signature against the chain (spec §6.4):
    /// the signer must have been owner or admin while the newest epoch in
    /// the keyring was current.
    pub fn verify_keyring(&self, record: &Record) -> Result<()> {
        if record.rtype() != RecordType::KEYR {
            return Err(acl("not a keyring record"));
        }
        let keyring = pb::Keyring::decode(open_plaintext(&self.doc_id, record)?.as_slice())
            .map_err(|err| CryptoError::InvalidInput(format!("keyring: {err}")))?;
        let epoch = keyring
            .epochs
            .iter()
            .map(|epoch| epoch.epoch)
            .max()
            .ok_or_else(|| acl("empty keyring"))?;
        let signer_id = signer_key_id(record)?;
        let grant = self
            .identity_grants
            .get(&signer_id)
            .ok_or_else(|| acl("signed by an identity the chain doesn't know"))?;
        verify_record(
            record,
            &self.doc_id,
            &Verifier::Identity(grant.public.clone()),
        )?;
        match grant.role_in(epoch) {
            Some(role) if role >= Role::Admin => Ok(()),
            _ => Err(acl(format!(
                "the signer wasn't owner or admin in epoch {epoch}"
            ))),
        }
    }

    fn member(&self, member_id: &KeyId) -> Result<&Member> {
        self.members
            .get(member_id)
            .ok_or_else(|| acl("not a member"))
    }

    /// Moves the head past `record` and records what everyone may do now.
    fn advance(&mut self, record: &Record) {
        self.head_hash = *blake3::hash(&record.encode()).as_bytes();
        self.next_index += 1;
        let epoch = self.epoch;
        for (member_id, member) in &self.members {
            self.identity_grants
                .entry(*member_id)
                .or_insert_with(|| Grant {
                    public: member.identity.clone(),
                    roles: BTreeMap::new(),
                })
                .note(epoch, member.role);
        }
        for (device_id, device) in &self.devices {
            let Some(member) = self.members.get(&device.member_id) else {
                continue;
            };
            self.device_grants
                .entry(*device_id)
                .or_insert_with(|| Grant {
                    public: device.public,
                    roles: BTreeMap::new(),
                })
                .note(epoch, member.role);
        }
    }
}

/// Builds and signs a `Genesis` change (spec §8.2): `owner` becomes the
/// owner, with `device` as their first device, at epoch 1.
pub fn genesis_record(
    doc_id: &DocId,
    owner: &IdentityKey,
    owner_xwing_public_key: Vec<u8>,
    device: &DeviceKey,
    device_xwing_public_key: Vec<u8>,
    rng: &mut dyn Randomness,
) -> Result<Record> {
    let genesis = pb::Genesis {
        owner: Some(identity_pb(&owner.public(), owner_xwing_public_key)),
        device: Some(pb::Device {
            ed25519_public_key: device.public().0.to_vec(),
            xwing_public_key: device_xwing_public_key,
        }),
        epoch: 1,
    };
    change_record(
        doc_id,
        0,
        [0; 32],
        pb::acl_change::Change::Genesis(genesis),
        owner,
        rng,
    )
}

/// The protobuf `Identity` for a member's public keys.
pub fn identity_pb(identity: &IdentityPublic, xwing_public_key: Vec<u8>) -> pb::Identity {
    pb::Identity {
        ed25519_public_key: identity.ed25519().to_vec(),
        mldsa65_public_key: identity.mldsa65().to_vec(),
        xwing_public_key,
    }
}

fn change_record(
    doc_id: &DocId,
    index: u64,
    prev_hash: [u8; 32],
    change: pb::acl_change::Change,
    signer: &IdentityKey,
    rng: &mut dyn Randomness,
) -> Result<Record> {
    let change = pb::AclChange {
        prev_hash: prev_hash.to_vec(),
        index,
        change: Some(change),
    };
    let mut object_id = [0u8; 32];
    rng.fill(&mut object_id)?;
    let record = RecordBuilder::new(RecordType::ACLC, ObjectId(object_id))
        .will_be_signed()
        .build(doc_id, change.encode_to_vec(), None)
        .map_err(|err| CryptoError::InvalidInput(err.to_string()))?;
    sign_record(record, doc_id, Signer::Identity(signer), rng)
}

/// The role spec §7.3 requires to sign a record of `rtype` in `stream`.
fn required_role(rtype: RecordType, stream: pb::Stream) -> Role {
    if rtype == RecordType::UPDT && stream == pb::Stream::Discussion {
        Role::Commenter
    } else {
        Role::Editor
    }
}

/// Whether `signer` may give a member the role `new` (spec §8.3): the
/// owner always, an admin only below admin. `old` is `None` for a new member.
fn may_manage(signer: Role, old: Option<Role>, new: Role) -> bool {
    match signer {
        Role::Owner => true,
        Role::Admin => old.is_none_or(|old| old < Role::Admin) && new < Role::Admin,
        _ => false,
    }
}

/// Whether `signer` may remove a member with `role` (spec §8.3).
fn may_remove(signer: Role, role: Role) -> bool {
    role != Role::Owner && may_manage(signer, Some(role), Role::Viewer)
}

struct ParsedIdentity {
    identity: IdentityPublic,
    xwing: Vec<u8>,
}

fn parse_identity(identity: Option<&pb::Identity>) -> Result<ParsedIdentity> {
    let identity = identity.ok_or_else(|| acl("missing identity"))?;
    // Also rejects Ed25519 keys that aren't curve points.
    let public = IdentityPublic::from_pb(identity)?;
    if identity.xwing_public_key.len() != XWING_PUBLIC_KEY_LEN {
        return Err(CryptoError::InvalidKey("identity X-Wing public key"));
    }
    Ok(ParsedIdentity {
        identity: public,
        xwing: identity.xwing_public_key.clone(),
    })
}

fn parse_device(device: Option<&pb::Device>, member_id: KeyId) -> Result<Device> {
    let device = device.ok_or_else(|| acl("missing device"))?;
    let public = DevicePublic::from_slice(&device.ed25519_public_key)?;
    // Reject keys that aren't curve points up front (weak keys are rejected
    // by `verify_strict` when verifying).
    ed25519_dalek::VerifyingKey::from_bytes(&public.0)
        .map_err(|_| CryptoError::InvalidKey("device Ed25519 public key"))?;
    if device.xwing_public_key.len() != XWING_PUBLIC_KEY_LEN {
        return Err(CryptoError::InvalidKey("device X-Wing public key"));
    }
    Ok(Device {
        member_id,
        public,
        xwing_public_key: device.xwing_public_key.clone(),
    })
}

fn decode_change(doc_id: &DocId, record: &Record) -> Result<pb::AclChange> {
    if record.rtype() != RecordType::ACLC {
        return Err(acl("not an ACLC record"));
    }
    if !record.flags().contains(RecordFlags::SIGNED) {
        return Err(acl("ACLC records must be signed"));
    }
    let change = pb::AclChange::decode(open_plaintext(doc_id, record)?.as_slice())
        .map_err(|err| acl(format!("malformed change: {err}")))?;
    if change.prev_hash.len() != 32 {
        return Err(acl("prev_hash must be 32 bytes"));
    }
    Ok(change)
}

/// The content of a `KEYR` or `ACLC` record, which are never encrypted.
fn open_plaintext(doc_id: &DocId, record: &Record) -> Result<Vec<u8>> {
    if record.flags().contains(RecordFlags::ENCRYPTED) {
        return Err(acl("KEYR and ACLC records must not be encrypted"));
    }
    record
        .open_content(doc_id, None, &Limits::default())
        .map_err(|err| CryptoError::InvalidInput(err.to_string()))
}

fn key_id(bytes: &[u8]) -> Result<KeyId> {
    KeyId::from_slice(bytes).ok_or_else(|| acl("malformed key id"))
}

fn acl(message: impl Into<String>) -> CryptoError {
    CryptoError::Acl(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::OsRandomness;
    use crate::{EpochKey, Suite1Sealer, XWingKeyPair};
    use pb::acl_change::Change;

    const DOC: DocId = DocId([0xAC; 16]);

    struct User {
        identity: IdentityKey,
        xwing: XWingKeyPair,
        device: DeviceKey,
        device_xwing: XWingKeyPair,
    }

    impl User {
        fn new() -> Self {
            let rng = &mut OsRandomness;
            Self {
                identity: IdentityKey::generate(rng).unwrap(),
                xwing: XWingKeyPair::generate(rng).unwrap(),
                device: DeviceKey::generate(rng).unwrap(),
                device_xwing: XWingKeyPair::generate(rng).unwrap(),
            }
        }

        fn id(&self) -> KeyId {
            self.identity.public().key_id()
        }

        fn device_id(&self) -> KeyId {
            self.device.public().key_id()
        }

        fn add(&self, role: Role) -> Change {
            Change::AddMember(pb::AddMember {
                member: Some(identity_pb(
                    &self.identity.public(),
                    self.xwing.public_bytes(),
                )),
                role: role.to_pb() as i32,
            })
        }

        fn add_device(&self) -> Change {
            Change::AddDevice(pb::AddDevice {
                member_id: self.id().0.to_vec(),
                device: Some(pb::Device {
                    ed25519_public_key: self.device.public().0.to_vec(),
                    xwing_public_key: self.device_xwing.public_bytes(),
                }),
            })
        }

        fn set_role(&self, role: Role) -> Change {
            Change::SetRole(pb::SetRole {
                member_id: self.id().0.to_vec(),
                role: role.to_pb() as i32,
            })
        }

        fn remove(&self) -> Change {
            Change::RemoveMember(pb::RemoveMember {
                member_id: self.id().0.to_vec(),
            })
        }

        /// An encrypted, signed update by this user's device.
        fn update(&self, epoch: u32, stream: pb::Stream) -> Record {
            let key = EpochKey::from_bytes(epoch, [7; 32]);
            let sealer = Suite1Sealer::new(&key, OsRandomness);
            let record = RecordBuilder::new(RecordType::UPDT, ObjectId::random().unwrap())
                .stream(stream)
                .will_be_signed()
                .build(&DOC, b"edit".to_vec(), Some(&sealer))
                .unwrap();
            sign_record(
                record,
                &DOC,
                Signer::Device(&self.device),
                &mut OsRandomness,
            )
            .unwrap()
        }
    }

    fn rotate(epoch: u32) -> Change {
        Change::RotateEpoch(pb::RotateEpoch { epoch })
    }

    /// A chain with `owner` and the given changes; returns the state and records.
    fn chain(owner: &User, changes: Vec<(Change, &User)>) -> Result<(AclState, Vec<Record>)> {
        let rng = &mut OsRandomness;
        let genesis = genesis_record(
            &DOC,
            &owner.identity,
            owner.xwing.public_bytes(),
            &owner.device,
            owner.device_xwing.public_bytes(),
            rng,
        )?;
        let mut state = AclState::genesis(&DOC, &genesis)?;
        let mut records = vec![genesis];
        for (change, signer) in changes {
            let record = state.next_change(change, &signer.identity, rng)?;
            state.apply(&record)?;
            records.push(record);
        }
        Ok((state, records))
    }

    #[test]
    fn genesis_creates_the_owner_and_device() {
        let owner = User::new();
        let (state, records) = chain(&owner, vec![]).unwrap();
        assert_eq!(state.owner(), owner.id());
        assert_eq!(state.role(&owner.id()), Some(Role::Owner));
        assert_eq!(state.epoch(), 1);
        assert_eq!(state.next_index(), 1);
        assert!(state.can_write(&owner.device_id(), pb::Stream::Main));
        assert_eq!(state.devices()[&owner.device_id()].member_id, owner.id());
        assert_eq!(
            state.head_hash(),
            *blake3::hash(&records[0].encode()).as_bytes()
        );
    }

    #[test]
    fn genesis_must_be_self_signed() {
        let owner = User::new();
        let impostor = User::new();
        let genesis = pb::Genesis {
            owner: Some(identity_pb(
                &owner.identity.public(),
                owner.xwing.public_bytes(),
            )),
            device: Some(pb::Device {
                ed25519_public_key: owner.device.public().0.to_vec(),
                xwing_public_key: owner.device_xwing.public_bytes(),
            }),
            epoch: 1,
        };
        let record = change_record(
            &DOC,
            0,
            [0; 32],
            Change::Genesis(genesis),
            &impostor.identity,
            &mut OsRandomness,
        )
        .unwrap();
        assert!(AclState::genesis(&DOC, &record).is_err());
    }

    #[test]
    fn members_join_add_devices_and_write_by_role() {
        let owner = User::new();
        let (editor, commenter, viewer) = (User::new(), User::new(), User::new());
        let (state, _) = chain(
            &owner,
            vec![
                (editor.add(Role::Editor), &owner),
                (editor.add_device(), &editor),
                (commenter.add(Role::Commenter), &owner),
                (commenter.add_device(), &commenter),
                (viewer.add(Role::Viewer), &owner),
                (viewer.add_device(), &owner),
            ],
        )
        .unwrap();
        assert!(state.can_write(&editor.device_id(), pb::Stream::Main));
        assert!(!state.can_write(&commenter.device_id(), pb::Stream::Main));
        assert!(state.can_write(&commenter.device_id(), pb::Stream::Discussion));
        assert!(!state.can_write(&viewer.device_id(), pb::Stream::Discussion));

        state
            .verify_content(&editor.update(1, pb::Stream::Main))
            .unwrap();
        state
            .verify_content(&commenter.update(1, pb::Stream::Discussion))
            .unwrap();
        assert!(
            state
                .verify_content(&commenter.update(1, pb::Stream::Main))
                .is_err()
        );
        assert!(
            state
                .verify_content(&viewer.update(1, pb::Stream::Discussion))
                .is_err()
        );
        assert!(
            state
                .verify_content(&User::new().update(1, pb::Stream::Main))
                .is_err(),
            "unknown device"
        );
        assert!(
            state
                .verify_content(&editor.update(2, pb::Stream::Main))
                .is_err(),
            "future epoch"
        );
    }

    #[test]
    fn admins_manage_only_below_admin() {
        let owner = User::new();
        let (admin, other_admin, editor) = (User::new(), User::new(), User::new());
        let base = vec![
            (admin.add(Role::Admin), &owner),
            (other_admin.add(Role::Admin), &owner),
            (editor.add(Role::Editor), &admin),
        ];
        let (state, _) = chain(&owner, base).unwrap();
        assert_eq!(state.role(&editor.id()), Some(Role::Editor));

        let try_change = |change: Change, signer: &User| {
            let record = state
                .next_change(change, &signer.identity, &mut OsRandomness)
                .unwrap();
            state.clone().apply(&record)
        };
        assert!(
            try_change(User::new().add(Role::Admin), &admin).is_err(),
            "admin can't add admins"
        );
        assert!(
            try_change(editor.set_role(Role::Admin), &admin).is_err(),
            "admin can't promote to admin"
        );
        assert!(
            try_change(other_admin.set_role(Role::Editor), &admin).is_err(),
            "admin can't demote admins"
        );
        assert!(
            try_change(other_admin.remove(), &admin).is_err(),
            "admin can't remove admins"
        );
        assert!(
            try_change(owner.remove(), &admin).is_err(),
            "nobody removes the owner"
        );
        assert!(
            try_change(owner.remove(), &owner).is_err(),
            "not even the owner"
        );
        assert!(
            try_change(owner.set_role(Role::Admin), &owner).is_err(),
            "the owner can't abdicate"
        );
        assert!(
            try_change(User::new().add(Role::Owner), &owner).is_err(),
            "only one owner"
        );
        assert!(
            try_change(User::new().add(Role::Viewer), &editor).is_err(),
            "editors manage nobody"
        );
        assert!(
            try_change(editor.add(Role::Viewer), &owner).is_err(),
            "already a member"
        );
        try_change(editor.set_role(Role::Viewer), &admin).unwrap();
        try_change(editor.remove(), &admin).unwrap();
        try_change(other_admin.remove(), &owner).unwrap();
    }

    #[test]
    fn devices_are_managed_by_their_member_or_their_managers() {
        let owner = User::new();
        let (admin, editor, stranger) = (User::new(), User::new(), User::new());
        let (state, _) = chain(
            &owner,
            vec![
                (admin.add(Role::Admin), &owner),
                (editor.add(Role::Editor), &owner),
            ],
        )
        .unwrap();
        let try_change = |change: Change, signer: &User| {
            let record = state
                .next_change(change, &signer.identity, &mut OsRandomness)
                .unwrap();
            state.clone().apply(&record)
        };
        try_change(editor.add_device(), &editor).unwrap();
        try_change(editor.add_device(), &admin).unwrap();
        assert!(try_change(admin.add_device(), &editor).is_err());
        let owner_device = Change::AddDevice(pb::AddDevice {
            member_id: owner.id().0.to_vec(),
            device: Some(pb::Device {
                ed25519_public_key: stranger.device.public().0.to_vec(),
                xwing_public_key: stranger.device_xwing.public_bytes(),
            }),
        });
        assert!(
            try_change(owner_device, &admin).is_err(),
            "admins can't add devices to the owner"
        );
        let duplicate = Change::AddDevice(pb::AddDevice {
            member_id: editor.id().0.to_vec(),
            device: Some(pb::Device {
                ed25519_public_key: owner.device.public().0.to_vec(),
                xwing_public_key: owner.device_xwing.public_bytes(),
            }),
        });
        assert!(
            try_change(duplicate, &owner).is_err(),
            "a device belongs to one member"
        );
        assert!(
            try_change(stranger.add(Role::Viewer), &stranger).is_err(),
            "non-members sign nothing"
        );
    }

    #[test]
    fn removal_requires_rotation_and_keeps_old_records_valid() {
        let owner = User::new();
        let editor = User::new();
        let (mut state, _) = chain(
            &owner,
            vec![
                (editor.add(Role::Editor), &owner),
                (editor.add_device(), &editor),
            ],
        )
        .unwrap();
        let before = editor.update(1, pb::Stream::Main);
        state.verify_content(&before).unwrap();

        let remove = state
            .next_change(editor.remove(), &owner.identity, &mut OsRandomness)
            .unwrap();
        state.apply(&remove).unwrap();
        assert!(state.rotation_required());
        assert!(
            !state.can_write(&owner.device_id(), pb::Stream::Main),
            "rotate first"
        );
        assert!(state.role(&editor.id()).is_none());
        assert!(!state.devices().contains_key(&editor.device_id()));

        let wrong = state
            .next_change(rotate(3), &owner.identity, &mut OsRandomness)
            .unwrap();
        assert!(
            state.clone().apply(&wrong).is_err(),
            "epochs increase by one"
        );
        let rotation = state
            .next_change(rotate(2), &owner.identity, &mut OsRandomness)
            .unwrap();
        state.apply(&rotation).unwrap();
        assert_eq!(state.epoch(), 2);
        assert!(state.can_write(&owner.device_id(), pb::Stream::Main));

        state.verify_content(&before).unwrap();
        assert!(
            state
                .verify_content(&editor.update(2, pb::Stream::Main))
                .is_err(),
            "removed"
        );
        state
            .verify_content(&owner.update(2, pb::Stream::Main))
            .unwrap();
    }

    #[test]
    fn rejected_changes_leave_the_state_unchanged() {
        let owner = User::new();
        let (admin, editor) = (User::new(), User::new());
        let (mut state, _) = chain(
            &owner,
            vec![
                (admin.add(Role::Admin), &owner),
                (editor.add(Role::Editor), &owner),
                (editor.add_device(), &editor),
            ],
        )
        .unwrap();
        let snapshot = |state: &AclState| {
            (
                state.members().clone(),
                state.devices().clone(),
                state.owner(),
                state.epoch(),
                state.next_index(),
                state.head_hash(),
                state.rotation_required(),
            )
        };
        let before = snapshot(&state);
        let rejected = [
            (admin.set_role(Role::Owner), &admin),
            (owner.set_role(Role::Viewer), &owner),
            (editor.add(Role::Viewer), &owner),
            (admin.remove(), &admin),
            (rotate(5), &owner),
            (User::new().add(Role::Viewer), &editor),
        ];
        for (change, signer) in rejected {
            let record = state
                .next_change(change, &signer.identity, &mut OsRandomness)
                .unwrap();
            assert!(state.apply(&record).is_err());
            assert_eq!(snapshot(&state), before);
        }
        assert!(
            state
                .verify_content(&editor.update(1, pb::Stream::Main))
                .is_ok()
        );
    }

    #[test]
    fn ownership_transfers_to_one_member() {
        let owner = User::new();
        let heir = User::new();
        let (state, _) = chain(
            &owner,
            vec![
                (heir.add(Role::Editor), &owner),
                (heir.set_role(Role::Owner), &owner),
            ],
        )
        .unwrap();
        assert_eq!(state.owner(), heir.id());
        assert_eq!(state.role(&heir.id()), Some(Role::Owner));
        assert_eq!(state.role(&owner.id()), Some(Role::Admin));
        let owners = state
            .members()
            .values()
            .filter(|member| member.role == Role::Owner)
            .count();
        assert_eq!(owners, 1);

        let (state, _) = chain(&owner, vec![(heir.add(Role::Admin), &owner)]).unwrap();
        let usurp = state
            .next_change(
                heir.set_role(Role::Owner),
                &heir.identity,
                &mut OsRandomness,
            )
            .unwrap();
        assert!(
            state.clone().apply(&usurp).is_err(),
            "only the owner transfers ownership"
        );
    }

    #[test]
    fn replay_checks_order_links_and_forks() {
        let owner = User::new();
        let (editor, viewer) = (User::new(), User::new());
        let (state, records) = chain(
            &owner,
            vec![
                (editor.add(Role::Editor), &owner),
                (viewer.add(Role::Viewer), &owner),
            ],
        )
        .unwrap();

        // Any order, duplicates allowed.
        let mut shuffled = vec![
            records[2].clone(),
            records[0].clone(),
            records[1].clone(),
            records[1].clone(),
        ];
        let replayed = AclState::replay(&DOC, &shuffled).unwrap();
        assert_eq!(replayed.members(), state.members());
        assert_eq!(replayed.head_hash(), state.head_hash());

        // A gap.
        assert!(AclState::replay(&DOC, [&records[0], &records[2]]).is_err());

        // A fork: a different change with index 2.
        let (_, other) = chain(&owner, vec![]).unwrap();
        let mut forked = AclState::genesis(&DOC, &records[0]).unwrap();
        forked.apply(&records[1]).unwrap();
        let fork = forked
            .next_change(
                User::new().add(Role::Viewer),
                &owner.identity,
                &mut OsRandomness,
            )
            .unwrap();
        shuffled.push(fork);
        assert_eq!(
            AclState::replay(&DOC, &shuffled).unwrap_err(),
            CryptoError::AclFork(2)
        );

        // A change that doesn't link to the head.
        let mut unlinked = AclState::genesis(&DOC, &other[0]).unwrap();
        assert!(unlinked.apply(&records[1]).is_err());

        // A different document.
        assert!(AclState::replay(&DocId([0; 16]), &records).is_err());
    }

    #[test]
    fn keyrings_must_come_from_admins_of_their_epoch() {
        let owner = User::new();
        let (admin, editor) = (User::new(), User::new());
        let (state, _) = chain(
            &owner,
            vec![
                (admin.add(Role::Admin), &owner),
                (editor.add(Role::Editor), &owner),
            ],
        )
        .unwrap();
        let key = EpochKey::from_bytes(1, [9; 32]);
        let wrap =
            crate::wrap_hpke(&DOC, &key, &editor.xwing.public_bytes(), &mut OsRandomness).unwrap();
        let keyring = pb::Keyring {
            epochs: vec![pb::EpochKeys {
                epoch: 1,
                wraps: vec![wrap],
            }],
        };
        let sign = |user: &User| {
            let record =
                crate::keyring_record(&DOC, ObjectId::random().unwrap(), &keyring, true).unwrap();
            sign_record(
                record,
                &DOC,
                Signer::Identity(&user.identity),
                &mut OsRandomness,
            )
            .unwrap()
        };
        state.verify_keyring(&sign(&owner)).unwrap();
        state.verify_keyring(&sign(&admin)).unwrap();
        assert!(state.verify_keyring(&sign(&editor)).is_err());
        assert!(state.verify_keyring(&sign(&User::new())).is_err());
    }

    #[test]
    fn key_providers_trust_only_authorized_keyrings() {
        use imaginarium_format::{FileHeader, KeyProvider};
        let owner = User::new();
        let editor = User::new();
        let (_, mut records) = chain(&owner, vec![(editor.add(Role::Editor), &owner)]).unwrap();
        let key = EpochKey::from_bytes(1, [9; 32]);
        let wrap =
            crate::wrap_hpke(&DOC, &key, &editor.xwing.public_bytes(), &mut OsRandomness).unwrap();
        let keyring = pb::Keyring {
            epochs: vec![pb::EpochKeys {
                epoch: 1,
                wraps: vec![wrap],
            }],
        };
        let unsigned =
            crate::keyring_record(&DOC, ObjectId::random().unwrap(), &keyring, true).unwrap();
        let header = FileHeader::new(DOC, true);
        let provider = || {
            crate::RecipientKeys::new(
                XWingKeyPair::from_private_bytes(&editor.xwing.private_bytes()).unwrap(),
                owner.identity.public().key_id(),
            )
        };

        // Planted by someone who isn't an admin: rejected, not silently used.
        let planted = sign_record(
            unsigned.clone(),
            &DOC,
            Signer::Identity(&editor.identity),
            &mut OsRandomness,
        )
        .unwrap();
        let mut with_planted = records.clone();
        with_planted.push(planted);
        assert!(provider().opener(&header, &with_planted).is_err());
        let mut with_unsigned = records.clone();
        with_unsigned.push(unsigned.clone());
        assert!(provider().opener(&header, &with_unsigned).is_err());

        let signed = sign_record(
            unsigned,
            &DOC,
            Signer::Identity(&owner.identity),
            &mut OsRandomness,
        )
        .unwrap();
        records.push(signed);
        assert!(provider().opener(&header, &records).is_ok());

        // The same chain, but the reader expects another owner (ADR-0018).
        let expecting_editor = crate::RecipientKeys::new(
            XWingKeyPair::from_private_bytes(&editor.xwing.private_bytes()).unwrap(),
            editor.identity.public().key_id(),
        )
        .opener(&header, &records);
        assert!(expecting_editor.is_err());
    }

    #[test]
    fn the_genesis_owner_survives_a_transfer() {
        let (owner, admin) = (User::new(), User::new());
        let (state, _) = chain(
            &owner,
            vec![
                (admin.add(Role::Admin), &owner),
                (admin.set_role(Role::Owner), &owner),
            ],
        )
        .unwrap();
        assert_eq!(state.owner(), admin.id());
        assert_eq!(state.genesis_owner(), owner.id());
    }
}
