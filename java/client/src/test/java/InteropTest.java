// Exercises actual JNI calls, concurrent callers, no-path/error distinction and lifecycle.
import org.multimodalmatsim.router.NativeRouter;
import org.multimodalmatsim.router.matsim.MatsimRouter;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.concurrent.Callable;
import java.util.concurrent.Executors;

public class InteropTest {
    public static void main(String[] args) throws Exception {
        var router = new NativeRouter(Path.of(args[0]), Path.of(args[1]));
        try (router) {
            var result = router.routeJson("A", "B", "07:59:00");
            require(result.contains("\"arrival\":29400000"), result);
            require(
                    router.routeJson("A", "C", "07:59:00").contains("\"result\":null"),
                    "missing no-path null");
            require(
                    router.routeJson("bad", "B", "07:59:00").contains("\"ok\":false"),
                    "missing error");
            require(router.metadataJson(true).contains("\"id\":\"A\""), "missing original IDs");
            var pool = Executors.newFixedThreadPool(12);
            try {
                var tasks = new ArrayList<Callable<String>>();
                for (int i = 0; i < 120; i++)
                    tasks.add(() -> router.routeJson("A", "B", "07:59:00"));
                for (var future : pool.invokeAll(tasks))
                    require(result.equals(future.get()), "concurrent mismatch");
            } finally {
                pool.shutdownNow();
            }
        }
        router.close();
        try {
            router.routeJson("A", "B", "07:59:00");
            throw new AssertionError("closed handle accepted");
        } catch (IllegalStateException expected) {
        }
        try (var session =
                MatsimRouter.load(
                        Path.of(args[0]).resolveSibling("matsim_config.xml"), Path.of(args[1]))) {
            session.awaitReady();
            require(session.isReady(), "startup not ready");
            require(
                    session.metadataJson(false).contains("\"wait_per_hour\":2.0"),
                    "MATSim costs not imported");
            var pool = Executors.newFixedThreadPool(12);
            try {
                var calls = new ArrayList<Callable<String>>();
                for (int i = 0; i < 120; i++) {
                    var worker = session.worker();
                    calls.add(() -> worker.routeJson("A", "B", "07:59:00"));
                }
                for (var result : pool.invokeAll(calls))
                    require(result.get().contains("\"arrival\":29400000"), "async route mismatch");
                require(
                        session.routeCoordinatesJson(0, 0, 1000, 0, "07:59:00")
                                .contains("\"arrival\":29400000"),
                        "coordinate route mismatch");
            } finally {
                pool.shutdownNow();
            }
        }
        var pending =
                MatsimRouter.load(
                        Path.of(args[0]).resolveSibling("matsim_config.xml"), Path.of(args[1]));
        pending.close();
        try {
            pending.awaitReady();
            throw new AssertionError("closed startup accepted");
        } catch (IllegalStateException expected) {
        }
        System.out.println(
                "Java native interop passed (240 concurrent queries, 12 workers; async startup and"
                        + " MATSim config)");
    }

    private static void require(boolean valid, String message) {
        if (!valid) throw new AssertionError(message);
    }
}
