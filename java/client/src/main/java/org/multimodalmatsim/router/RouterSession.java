// Runs loading in a bounded background executor and shares one native engine across thread views.
package org.multimodalmatsim.router;

import java.lang.ref.Cleaner;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Supplier;

public final class RouterSession implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();
    private static final ExecutorService LOADERS =
            Executors.newFixedThreadPool(
                    2,
                    r -> {
                        var t = new Thread(r, "mmrouter-load");
                        t.setDaemon(true);
                        return t;
                    });

    private static final class State implements Runnable {
        final AtomicBoolean closed = new AtomicBoolean();
        final CompletableFuture<NativeRouter> future;

        State(CompletableFuture<NativeRouter> future) {
            this.future = future;
        }

        public void run() {
            if (closed.compareAndSet(false, true))
                future.whenComplete(
                        (router, error) -> {
                            if (router != null) router.close();
                        });
        }
    }

    private final State state;
    private final Cleaner.Cleanable cleanable;

    private RouterSession(CompletableFuture<NativeRouter> future) {
        state = new State(future);
        cleanable = CLEANER.register(this, state);
    }

    static RouterSession start(Supplier<NativeRouter> loader) {
        return new RouterSession(CompletableFuture.supplyAsync(loader, LOADERS));
    }

    private NativeRouter client() {
        if (state.closed.get()) throw new IllegalStateException("Router session is closed");
        return state.future.join();
    }

    public boolean isReady() {
        return !state.closed.get()
                && state.future.isDone()
                && !state.future.isCompletedExceptionally();
    }

    public RouterSession awaitReady() {
        client();
        return this;
    }

    public String routeJson(String from, String to, String departure) {
        return client().routeJson(from, to, departure);
    }

    public String batchJson(String queries) {
        return client().batchJson(queries);
    }

    public String weightedBatchJson(String queries) {
        return client().weightedBatchJson(queries);
    }

    public String routeCoordinatesJson(
            double x, double y, double toX, double toY, String departure) {
        return client().routeCoordinatesJson(x, y, toX, toY, departure);
    }

    public String metadataJson(boolean entities) {
        return client().metadataJson(entities);
    }

    public Worker worker() {
        return new Worker(this);
    }

    @Override
    public void close() {
        cleanable.clean();
    }

    /** A view has no independent engine/pool and does not close its owner's resources. */
    public static final class Worker {
        private final RouterSession owner;

        private Worker(RouterSession owner) {
            this.owner = owner;
        }

        public String routeJson(String from, String to, String departure) {
            return owner.routeJson(from, to, departure);
        }

        public String batchJson(String queries) {
            return owner.batchJson(queries);
        }

        public String routeCoordinatesJson(
                double x, double y, double toX, double toY, String departure) {
            return owner.routeCoordinatesJson(x, y, toX, toY, departure);
        }
    }
}
