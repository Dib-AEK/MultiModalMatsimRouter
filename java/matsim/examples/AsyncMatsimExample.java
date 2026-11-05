// Starts MATSim-config preprocessing asynchronously and shares one engine across Java workers.
import org.multimodalmatsim.router.matsim.MatsimRouter;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.concurrent.Callable;
import java.util.concurrent.Executors;

public class AsyncMatsimExample {
    public static void main(String[] args) throws Exception {
        try (var router =
                args.length > 1
                        ? MatsimRouter.load(Path.of(args[0]), Path.of(args[1]))
                        : MatsimRouter.load(Path.of(args[0]))) {
            System.out.println(
                    "The router loads in the background while the application continues.");
            router.awaitReady(); // Optional; the first query waits if necessary.
            var pool = Executors.newFixedThreadPool(12);
            try {
                var requests = new ArrayList<Callable<String>>();
                for (int i = 0; i < 12; i++) {
                    var worker = router.worker(); // A view of the shared engine.
                    requests.add(() -> worker.routeJson("A", "B", "07:59:00"));
                }
                var results = pool.invokeAll(requests);
                System.out.println(results.get(0).get());
            } finally {
                pool.shutdownNow();
            }
        }
    }
}
