// Offers a small MATSim-config entry point while leaving loading, search and ownership in shared
// modules.
package org.multimodalmatsim.router.matsim;

import org.multimodalmatsim.router.NativeRouter;
import org.multimodalmatsim.router.RouterSession;

import java.nio.file.Path;

public final class MatsimRouter {
    private MatsimRouter() {}

    public static RouterSession load(Path config) {
        return load(config, null);
    }

    public static RouterSession start(Path config) {
        return load(config);
    }

    /**
     * Returns immediately; config-relative schedule paths and supported router settings are
     * imported.
     */
    public static RouterSession load(Path config, Path library) {
        return NativeRouter.start(config, library, "matsim_config", "{}");
    }

    public static RouterSession start(Path config, Path library) {
        return load(config, library);
    }

    /**
     * Optional partial overrides, e.g. {"execution":{"threads":12}}; no timetable is copied per
     * worker.
     */
    public static RouterSession load(Path config, Path library, String overridesJson) {
        return NativeRouter.start(config, library, "matsim_config", overridesJson);
    }
}
