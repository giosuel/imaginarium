// SPDX-License-Identifier: AGPL-3.0-or-later

//! An opened file kept open for saving: each change is appended as an
//! `UPDT` record and committed with a new index (spec §9.3), so every
//! change reaches the file on its own (PLAN §14, "Saving").

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use imaginarium_format::{
    DocId, Durable, ObjectId, Reader, Record, RecordBuilder, RecordType, Writer, pb,
};
use imaginarium_model::CODEC;

use crate::{
    Credentials, IndexedRecords, NativeError, Opened, Result, open_reader, read_acl, read_document,
    read_meta, snapshot_record,
};

/// A document file open for saving into.
pub struct Session<F = File> {
    writer: Writer<F>,
    doc_id: DocId,
}

impl Session<File> {
    /// Opens the file at `path` to edit it: the document, and a session
    /// that saves its changes into the same file. Encrypted files can't be
    /// saved into yet (their keys aren't kept): [`NativeError::CantSave`].
    ///
    /// # Errors
    /// If the file can't be opened for writing, or read.
    pub fn open(path: &Path) -> Result<(Opened, Self)> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        Self::from_file(file)
    }
}

impl<F: Read + Write + Seek + Durable> Session<F> {
    /// Opens `file` to edit it (see [`Session::open`]).
    ///
    /// # Errors
    /// If it can't be read, or is encrypted.
    pub fn from_file(mut file: F) -> Result<(Opened, Self)> {
        // Without its key an encrypted file doesn't even open: say why.
        let mut header = [0u8; imaginarium_format::HEADER_LEN];
        file.seek(std::io::SeekFrom::Start(0))?;
        if file.read_exact(&mut header).is_ok()
            && imaginarium_format::FileHeader::decode(&header)
                .is_ok_and(|header| header.is_encrypted())
        {
            return Err(NativeError::CantSave("it is encrypted"));
        }
        let mut reader: Reader<F> = open_reader(file, Credentials::None)?;
        let records = IndexedRecords::new(&reader);
        let acl = read_acl(&mut reader)?;
        if acl.is_some() {
            return Err(NativeError::CantSave("it belongs to a Hub"));
        }
        let document = read_document(&mut reader, &records, acl.as_ref())?;
        let meta = read_meta(&mut reader)?;
        let header = *reader.header();
        let recovery = reader.recovery().cloned();
        let writer = Writer::resume(reader)?;
        let opened = Opened {
            document,
            header,
            meta,
            recovery,
            acl,
        };
        Ok((
            opened,
            Self {
                writer,
                doc_id: header.doc_id,
            },
        ))
    }

    /// Appends one update of the `main` stream (from
    /// `Document::export_updates_since`). It is in the file once
    /// committed.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn append_update(&mut self, update: Vec<u8>) -> Result<()> {
        let record: Record = RecordBuilder::new(RecordType::UPDT, ObjectId::random()?)
            .stream(pb::Stream::Main)
            .codec(CODEC)
            .compress(update.len() > 512)
            .build(&self.doc_id, update, None)?;
        self.writer.append(&record)?;
        Ok(())
    }

    /// Appends a snapshot of the whole document: the stream's head starts
    /// again from it, so the file opens without replaying every update
    /// (and a recovered file gets a sound head, spec §9.5).
    ///
    /// # Errors
    /// If it can't be written.
    pub fn append_snapshot(&mut self, snapshot: Vec<u8>) -> Result<()> {
        self.writer
            .append(&snapshot_record(&self.doc_id, snapshot, None)?)?;
        Ok(())
    }

    /// Commits what was appended: a new index and trailer, synced to disk.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn commit(&mut self) -> Result<()> {
        if self.writer.pending() > 0 {
            self.writer.commit(None)?;
        }
        Ok(())
    }

    /// How many records the stream's head has: its snapshot and the updates
    /// since (a new snapshot is due when there are many).
    pub fn head_len(&self) -> usize {
        self.writer
            .index()
            .head(&imaginarium_format::StreamKey::main())
            .map_or(0, |head| {
                head.update_ids.len() + usize::from(!head.snapshot_id.is_empty())
            })
    }
}
