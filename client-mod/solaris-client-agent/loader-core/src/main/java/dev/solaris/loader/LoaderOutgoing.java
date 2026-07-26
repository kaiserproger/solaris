package dev.solaris.loader;

public record LoaderOutgoing(
        Kind kind,
        byte[] bytes,
        LoaderActivatedContent activatedContent) {
    public enum Kind {
        REQUEST,
        ACKNOWLEDGEMENT
    }

    public LoaderOutgoing {
        bytes = bytes.clone();
        if (kind == Kind.ACKNOWLEDGEMENT && activatedContent == null) {
            throw new IllegalArgumentException(
                    "Loader acknowledgement requires activated content");
        }
        if (kind == Kind.REQUEST && activatedContent != null) {
            throw new IllegalArgumentException(
                    "Loader request cannot carry activated content");
        }
    }

    public LoaderOutgoing(Kind kind, byte[] bytes) {
        this(kind, bytes, null);
    }

    @Override
    public byte[] bytes() {
        return bytes.clone();
    }
}
