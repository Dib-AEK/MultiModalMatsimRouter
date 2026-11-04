// Exercises actual JNI calls, concurrent callers, no-path/error distinction and lifecycle.
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.concurrent.Callable;
import java.util.concurrent.Executors;
import org.multimodalmatsim.router.NativeRouter;

public class InteropTest {
    public static void main(String[] args) throws Exception {
        var router = new NativeRouter(Path.of(args[0]), Path.of(args[1]));
        try (router) {
            var result = router.routeJson("A", "B", "07:59:00");
            require(result.contains("\"arrival\":29400000"), result);
            require(router.routeJson("A", "C", "07:59:00").contains("\"result\":null"), "missing no-path null");
            require(router.routeJson("bad", "B", "07:59:00").contains("\"ok\":false"), "missing error");
            require(router.metadataJson(true).contains("\"id\":\"A\""), "missing original IDs");
            var pool = Executors.newFixedThreadPool(12);
            try {
                var tasks = new ArrayList<Callable<String>>();
                for (int i = 0; i < 120; i++) tasks.add(() -> router.routeJson("A", "B", "07:59:00"));
                for (var future : pool.invokeAll(tasks)) require(result.equals(future.get()), "concurrent mismatch");
            } finally { pool.shutdownNow(); }
        }
        router.close();
        try { router.routeJson("A", "B", "07:59:00"); throw new AssertionError("closed handle accepted"); }
        catch (IllegalStateException expected) { }
        System.out.println("Java native interop passed (120 queries, 12 workers)");
    }
    private static void require(boolean valid, String message) { if (!valid) throw new AssertionError(message); }
}
