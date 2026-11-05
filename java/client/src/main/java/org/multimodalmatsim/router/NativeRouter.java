// Provides a dependency-free Java client for the standalone shared Rust application API.
package org.multimodalmatsim.router;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.atomic.AtomicLong;

public final class NativeRouter implements AutoCloseable {
    private final AtomicLong handle;
    private static String loadedLibrary;

    /** Explicit library paths are portable; configuration input paths remain relative to TOML. */
    public NativeRouter(Path config, Path library) {
        this(config, library, "auto", "{}");
    }

    public NativeRouter(Path config) {
        this(config, null);
    }

    public NativeRouter(Path input, Path library, String format, String parametersJson) {
        loadLibrary(library);
        handle =
                new AtomicLong(
                        openInput(input.toAbsolutePath().toString(), format, parametersJson));
    }

    public static RouterSession load(Path file, Path library) {
        return start(file, library, "auto", "{}");
    }

    public static RouterSession load(Path file) {
        return load(file, null);
    }

    public static RouterSession start(Path file, String format, String parametersJson) {
        return start(file, null, format, parametersJson);
    }

    public static RouterSession start(
            Path file, Path library, String format, String parametersJson) {
        return RouterSession.start(() -> new NativeRouter(file, library, format, parametersJson));
    }

    private static synchronized void loadLibrary(Path library) {
        String override =
                System.getProperty("mmrouter.library", System.getenv("MMROUTER_JNI_LIBRARY"));
        if (library == null && override != null && !override.isBlank()) library = Path.of(override);
        if (library == null && loadedLibrary != null) return;
        String path = library == null ? "bundled" : library.toAbsolutePath().normalize().toString();
        if (loadedLibrary != null && !loadedLibrary.equals(path))
            throw new IllegalStateException("A different native router library is already loaded");
        if (loadedLibrary == null) {
            if (library != null) System.load(path);
            else {
                String name = System.mapLibraryName("matsim_pt_router");
                try (var stream = NativeRouter.class.getResourceAsStream("/native/" + name)) {
                    if (stream == null) System.loadLibrary("matsim_pt_router");
                    else {
                        Path directory = Files.createTempDirectory("mmrouter-java-");
                        Path binary = directory.resolve(name);
                        Files.copy(stream, binary);
                        directory.toFile().deleteOnExit();
                        binary.toFile().deleteOnExit();
                        System.load(binary.toString());
                    }
                } catch (java.io.IOException error) {
                    throw new IllegalStateException("Cannot extract native router library", error);
                }
            }
            if (protocolVersion() != 1)
                throw new IllegalStateException("Unsupported native router protocol");
            if (apiVersion() < 2)
                throw new IllegalStateException(
                        "Native library needs client API 2; rebuild with tools/build.py");
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
        return requestJson(
                "{\"op\":\"route\",\"from\":"
                        + quote(from)
                        + ",\"to\":"
                        + quote(to)
                        + ",\"departure\":"
                        + quote(departure)
                        + "}");
    }

    public String batchJson(String queriesJson) {
        return requestJson("{\"op\":\"batch\",\"queries\":" + queriesJson + "}");
    }

    public String weightedBatchJson(String queriesJson) {
        return requestJson("{\"op\":\"weighted_batch\",\"queries\":" + queriesJson + "}");
    }

    public String routeCoordinatesJson(
            double x, double y, double toX, double toY, String departure) {
        return requestJson(
                "{\"op\":\"route_coordinates\",\"origin\":["
                        + x
                        + ","
                        + y
                        + "],\"destination\":["
                        + toX
                        + ","
                        + toY
                        + "],\"departure\":"
                        + quote(departure)
                        + "}");
    }

    public String metadataJson(boolean includeEntities) {
        return requestJson("{\"op\":\"metadata\",\"include_entities\":" + includeEntities + "}");
    }

    private static String quote(String value) {
        var result = new StringBuilder("\"");
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c == '"' || c == '\\') result.append('\\').append(c);
            else if (c < 32) result.append(String.format("\\u%04x", (int) c));
            else result.append(c);
        }
        return result.append('"').toString();
    }

    @Override
    public void close() {
        long id = handle.getAndSet(0);
        if (id != 0) release(id);
    }

    private static native int protocolVersion();

    private static native int apiVersion();

    private static native long openConfig(String path);

    private static native long openInput(String path, String format, String parametersJson);

    private static native String request(long id, String command);

    private static native void release(long id);
}
