// Loads a standalone native router and prints a versioned journey response without MATSim.
import java.nio.file.Path;
import org.multimodalmatsim.router.NativeRouter;

public class JavaExample {
    public static void main(String[] args) {
        try (var router = new NativeRouter(Path.of(args[0]), Path.of(args[1]))) {
            System.out.println(router.metadataJson(false));
            System.out.println(router.routeJson("A", "B", "07:59:00"));
        }
    }
}
