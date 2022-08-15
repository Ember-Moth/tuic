use super::{
    stream::{RecvStream, SendStream, Stream as BiStream, StreamReg},
    task::TaskSource,
};
use bytes::Bytes;
use futures::{stream::SelectAll, Stream};
use quinn::{
    Datagrams, IncomingBiStreams, IncomingUniStreams, RecvStream as QuinnRecvStream,
    SendStream as QuinnSendStream,
};
use std::{
    io::Error as IoError,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

pub(crate) struct RawIncomingTasks {
    incoming: SelectAll<IncomingSource>,
    stream_reg: Arc<StreamReg>,
}

impl RawIncomingTasks {
    pub(crate) fn new(
        bi_streams: IncomingBiStreams,
        uni_streams: IncomingUniStreams,
        datagrams: Datagrams,
        stream_reg: Arc<StreamReg>,
    ) -> Self {
        let mut incoming = SelectAll::new();

        incoming.push(IncomingSource::BiStreams(bi_streams));
        incoming.push(IncomingSource::UniStreams(uni_streams));
        incoming.push(IncomingSource::Datagrams(datagrams));

        Self {
            incoming,
            stream_reg,
        }
    }
}

impl Stream for RawIncomingTasks {
    type Item = Result<TaskSource, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming)
            .poll_next(cx)
            .map_ok(|src| match src {
                IncomingItem::BiStream((send, recv)) => TaskSource::BiStream(BiStream::new(
                    SendStream::new(send, self.stream_reg.as_ref().clone()),
                    RecvStream::new(recv, self.stream_reg.as_ref().clone()),
                )),
                IncomingItem::UniStream(recv) => {
                    TaskSource::UniStream(RecvStream::new(recv, self.stream_reg.as_ref().clone()))
                }
                IncomingItem::Datagram(datagram) => TaskSource::Datagram(datagram),
            })
            .map_err(IoError::from)
    }
}

enum IncomingSource {
    BiStreams(IncomingBiStreams),
    UniStreams(IncomingUniStreams),
    Datagrams(Datagrams),
}

impl Stream for IncomingSource {
    type Item = Result<IncomingItem, IoError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            IncomingSource::BiStreams(bi_streams) => Pin::new(bi_streams)
                .poll_next(cx)
                .map_ok(IncomingItem::BiStream)
                .map_err(IoError::from),
            IncomingSource::UniStreams(uni_streams) => Pin::new(uni_streams)
                .poll_next(cx)
                .map_ok(IncomingItem::UniStream)
                .map_err(IoError::from),
            IncomingSource::Datagrams(datagrams) => Pin::new(datagrams)
                .poll_next(cx)
                .map_ok(IncomingItem::Datagram)
                .map_err(IoError::from),
        }
    }
}

enum IncomingItem {
    BiStream((QuinnSendStream, QuinnRecvStream)),
    UniStream(QuinnRecvStream),
    Datagram(Bytes),
}
