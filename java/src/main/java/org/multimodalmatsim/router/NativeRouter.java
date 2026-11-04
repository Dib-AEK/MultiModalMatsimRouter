// Provides a dependency-free Java client for the standalone shared Rust application API.
package org.multimodalmatsim.router;

import java.nio.file.Path;
import java.util.concurrent.atomic.AtomicLong;

public final class NativeRouter implements AutoCloseable {
    private final AtomicLong handle;
    private static String loadedLibrary;

    /** Explicit library paths are portable; configuration input paths remain relative to TOML. */
    public NativeRouter(Path config, Path library) {
        load(library);
        handle = new AtomicLong(openConfig(config.toAbsolutePath().toString()));
    }

    private static synchronized void load(Path library) {
        String path = library.toAbsolutePath().normalize().toString();
        if (loadedLibrary != null && !loadedLibrary.equals(path))
            throw new IllegalStateException("A different native router library is already loaded");
        if (loadedLibrary == null) {
            System.load(path);
            if (protocolVersion() != 1) throw new IllegalStateException("Unsupported native router protocol");
            loadedLibrary = path;
        }
    }

    /** Returns the version-1 JSON envelope. No-path is a successful null result. */
    public String requestJson(String command) {
        long id = handle.get();
        if (id == 0) throw new IllegalStateException("Router is closed");
        return request(id, command);
    }

    public String routeJson(String from, String to, String departure) {
        return requestJson("{\"op\":\"route\",\"from\":" + quote(from)
                + ",\"to\":" + quote(to) + ",\"departure\":" + quote(departure) + "}");
    }

    public String batchJson(String queriesJson) {
        return requestJson("{\"op\":\"batch\",\"queries\":" + queriesJson + "}");
    }

    public String weightedBatchJson(String queriesJson) {
        return requestJson("{\"op\":\"weighted_batch\",\"queries\":" + queriesJson + "}");
    }

    public String metadataJson(boolean includeEntities) {
        return requestJson("{\"op\":\"metadata\",\"include_entities\":" + includeEntities + "}");
    }

    private static String quote(String value) {
        var result = new StringBuilder("\"");
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c == '"' || c == '\\') result.append('\\').append(c);
            else if (c < 32) result.append(String.format("\\u%04x", (int)c));
            else result.append(c);
        }
        return result.append('"').toString();
    }

    @Override public void close() {
        long id = handle.getAndSet(0);
        if (id != 0) release(id);
    }

    private static native int protocolVersion();
    private static native long openConfig(String path);
    private static native String request(long id, String command);
    private static native void release(long id);
}
