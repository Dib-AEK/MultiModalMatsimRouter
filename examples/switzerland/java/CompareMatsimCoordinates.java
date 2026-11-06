// Measures the MATSim coordinate boundary and transit-leg reconstruction with identical native search data.
package org.eqasim.switzerland.ch_cmdp.rustRouting;

import com.fasterxml.jackson.databind.ObjectMapper;
import ch.sbb.matsim.config.SwissRailRaptorConfigGroup;
import org.eqasim.core.components.raptor.EqasimRaptorConfigGroup;
import org.matsim.api.core.v01.*;
import org.matsim.api.core.v01.population.*;
import org.matsim.core.config.*;
import org.matsim.core.router.*;
import org.matsim.core.scenario.ScenarioUtils;
import org.matsim.facilities.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;

public final class CompareMatsimCoordinates {
    private CompareMatsimCoordinates() {}
    static Object invoke(boolean nativeCoordinates, boolean legs, RustRoutingRuntime runtime,
            RustPtRoutingModule adapter, RoutingRequest request) {
        if (legs) return adapter.transitLegs(request);
        var ready = runtime.prepared();
        return nativeCoordinates ? runtime.routeCoordinates(ready.handle(), request)
                : LegacyRustEndpoints.route(ready.snapshot(),runtime,ready.handle(),request);
    }
    static Object signature(Object result) {
        if (result == null) return List.of();
        if (!(result instanceof List<?> elements)) return result;
        List<String> signature = new ArrayList<>();
        for (var element : elements) {
            Leg leg = (Leg) element;
            var route = leg.getRoute();
            signature.add(leg.getMode()+"|"+leg.getDepartureTime()+"|"+leg.getTravelTime()+"|"
                    +route.getStartLinkId()+"|"+route.getEndLinkId()+"|"+route.getRouteDescription()
                    +"|"+route.getDistance()+"|"+leg.getAttributes().getAttribute("totalRouteCost"));
        }
        return signature;
    }
    static boolean equal(Object a, Object b) {
        return a instanceof double[] x && b instanceof double[] y ? Arrays.equals(x,y) : a.equals(b);
    }
    public static void main(String[] args) throws Exception {
        if (args.length != 6 && args.length != 7) throw new IllegalArgumentException("config network schedule queries library report [native-only]");
        var mapper = new ObjectMapper();
        var config = ConfigUtils.loadConfig(args[0],new RustRoutingConfigGroup(),
                new EqasimRaptorConfigGroup(),new SwissRailRaptorConfigGroup());
        config.global().setNumberOfThreads(8);
        var scenario = ScenarioUtils.createScenario(config);
        new org.matsim.core.network.io.MatsimNetworkReader(scenario.getNetwork()).readFile(args[1]);
        new org.matsim.pt.transitSchedule.api.TransitScheduleReader(scenario).readFile(args[2]);
        var settings = ConfigUtils.addOrGetModule(config,RustRoutingConfigGroup.class);
        settings.enabled=true;settings.libraryPath=Path.of(args[4]).toAbsolutePath().toString();
        List<RoutingRequest> requests=new ArrayList<>();
        var factory=FacilitiesUtils.createActivityFacilities().getFactory();
        var link=scenario.getNetwork().getLinks().keySet().iterator().next();
        for (var q : mapper.readTree(Path.of(args[3]).toFile())) {
            var o=q.get("origin");var d=q.get("destination");
            var from=factory.createActivityFacility(Id.create("o"+requests.size(),ActivityFacility.class),new Coord(o.get(0).asDouble(),o.get(1).asDouble()),link);
            var to=factory.createActivityFacility(Id.create("d"+requests.size(),ActivityFacility.class),new Coord(d.get(0).asDouble(),d.get(1).asDouble()),link);
            requests.add(DefaultRoutingRequest.withoutAttributes(from,to,
                    org.matsim.core.utils.misc.Time.parseTime(q.get("departure").asText()),null));
        }
        Map<String,Object> report=new LinkedHashMap<>();report.put("workers",8);report.put("queries",requests.size());
        try (var runtime=new RustRoutingRuntime(scenario,settings);var pool=Executors.newFixedThreadPool(8)) {
            var snapshot=runtime.prepared().snapshot();
            if (!NativePtRouter.hasCoordinates) throw new IllegalStateException("new coordinate library required");
            var walk=config.routing().getOrCreateModeRoutingParams("walk");
            report.put("walk",Map.of("configured_speed",walk.getTeleportedModeSpeed(),
                    "configured_beeline_factor",walk.getBeelineDistanceFactor(),
                    "effective_raptor_speed",snapshot.parameters.getBeelineWalkSpeed(),
                    "exported_native_speed",mapper.readTree(snapshot.costsJson()).get("_coordinates").get("speed").asDouble()));
            var adapter=new RustPtRoutingModule(runtime,null);
            for (boolean legs : args.length == 7 ? new boolean[]{true} : new boolean[]{false,true}) {
                List<List<Double>> times=List.of(new ArrayList<>(),new ArrayList<>());
                Object[] reference=null;
                for (int pass=-1;pass<3;pass++) {
                    for (int variant : args.length == 7 ? new int[]{1} : pass%2==0 ? new int[]{0,1} : new int[]{1,0}) {
                        boolean nativeCoordinates=variant==1;
                        // Workers are idle here; switch only the adapter capability branch, not the engine or timetable.
                        NativePtRouter.hasCoordinates=nativeCoordinates;
                        List<Callable<Object>> tasks=new ArrayList<>();
                        int count=requests.size();
                        for (int i=0;i<count;i++) {
                            var request=requests.get(i);
                            tasks.add(()->invoke(nativeCoordinates,legs,runtime,adapter,request));
                        }
                        long start=System.nanoTime();var results=pool.invokeAll(tasks);
                        for(var future : results) future.get();
                        double elapsed=(System.nanoTime()-start)/1e9;
                        Object[] values=new Object[count];
                        for(int i=0;i<count;i++) values[i]=signature(results.get(i).get());
                        if(pass>=0) {
                            times.get(variant).add(elapsed);
                            if(reference==null) reference=values;
                            else for(int i=0;i<count;i++) if(!equal(reference[i],values[i]))
                                throw new IllegalStateException("Different route at query "+i+", legs="+legs);
                            System.out.println("MEASURE legs="+legs+" native="+nativeCoordinates+" seconds="+elapsed);
                        }
                    }
                }
                report.put(legs?"with_matsim_legs":"coordinate_payload",Map.of("seconds",times,"all_equal",true,
                    "itinerary_sha256", HexFormat.of().formatHex(java.security.MessageDigest.getInstance("SHA-256").digest(mapper.writeValueAsBytes(reference)))));
            }
            NativePtRouter.hasCoordinates=true;
        }
        Files.writeString(Path.of(args[5]),mapper.writerWithDefaultPrettyPrinter().writeValueAsString(report));
        System.out.println(mapper.writeValueAsString(report));
    }
}
