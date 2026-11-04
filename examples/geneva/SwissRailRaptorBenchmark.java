// Benchmarks the SwissRailRaptor core on the same controlled stop-to-stop requests as the Rust
// engine.
package ch.sbb.matsim.routing.pt.raptor;

import ch.sbb.matsim.config.SwissRailRaptorConfigGroup;
import com.fasterxml.jackson.databind.*;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import org.matsim.api.core.v01.Id;
import org.matsim.core.config.ConfigUtils;
import org.matsim.core.network.io.MatsimNetworkReader;
import org.matsim.core.scenario.ScenarioUtils;
import org.matsim.pt.transitSchedule.api.*;

public class SwissRailRaptorBenchmark {
  static boolean weightedProfile;

  record Request(
      String id,
      TransitStopFacility from,
      TransitStopFacility to,
      double departure,
      List<InitialStop> access,
      List<InitialStop> egress) {}

  static double seconds(String time) {
    var p = time.split(":");
    return Double.parseDouble(p[0]) * 3600
        + Double.parseDouble(p[1]) * 60
        + Double.parseDouble(p[2]);
  }

  static SwissRailRaptorCore core(SwissRailRaptorData data) {
    return new SwissRailRaptorCore(
        data,
        new DefaultRaptorInVehicleCostCalculator(),
        data.config.isUseModeToModeTransferPenalty()
            ? new ModeSpecificTransferCostCalculator()
            : new DefaultRaptorTransferCostCalculator());
  }

  static RaptorRoute route(SwissRailRaptorCore core, Request q, RaptorParameters params) {
    return core.calcLeastCostRoute(q.departure, q.from, q.to, q.access, q.egress, params, null);
  }

  static Map<String, Object> describe(RaptorRoute result) {
    if (result == null || !Double.isFinite(result.getTotalCosts())) return null;
    var legs = new ArrayList<Map<String, Object>>();
    Map<String, Object> access = null, egress = null;
    int boardings = 0;
    double arrival = Double.NaN;
    for (var initial : result.getParts())
      for (var part = initial; part != null; part = part.chainedPart) {
        arrival = part.arrivalTime;
        if (part.line == null) {
          if (part.fromStop == null && part.toStop != null)
            access =
                Map.of(
                    "stop",
                    part.toStop.getId().toString(),
                    "duration",
                    Math.round((part.arrivalTime - part.depTime) * 1000));
          else if (part.toStop == null && part.fromStop != null)
            egress =
                Map.of(
                    "stop",
                    part.fromStop.getId().toString(),
                    "duration",
                    Math.round((part.arrivalTime - part.depTime) * 1000));
          else if (weightedProfile && part.fromStop != null && part.toStop != null)
            legs.add(
                Map.of(
                    "kind",
                    "walk",
                    "from",
                    part.fromStop.getId().toString(),
                    "to",
                    part.toStop.getId().toString(),
                    "departure",
                    Math.round(part.depTime * 1000),
                    "arrival",
                    Math.round(part.arrivalTime * 1000)));
          continue;
        }
        boardings++;
        legs.add(
            Map.of(
                "kind",
                "transit",
                "from",
                part.fromStop.getId().toString(),
                "to",
                part.toStop.getId().toString(),
                "line",
                part.line.getId().toString(),
                "route",
                part.route.getId().toString(),
                "departure",
                Math.round(part.boardingTime * 1000),
                "arrival",
                Math.round(part.arrivalTime * 1000),
                "vehicle_departure",
                Math.round(part.vehicleDepTime * 1000)));
      }
    if (boardings == 0) return null;
    var description = new LinkedHashMap<String, Object>();
    description.put("arrival", Math.round(arrival * 1000));
    description.put("boardings", boardings);
    description.put("legs", legs);
    description.put("generalized_cost", result.getTotalCosts());
    description.put("access", access);
    description.put("egress", egress);
    return description;
  }

  static List<InitialStop> endpoints(
      JsonNode values, TransitStopFacility fallback, TransitSchedule schedule) {
    if (values == null) return List.of(new InitialStop(fallback, 0, 0, 0, "walk"));
    var result = new ArrayList<InitialStop>();
    for (var e : values) {
      var stop =
          schedule
              .getFacilities()
              .get(Id.create(e.get("stop").asText(), TransitStopFacility.class));
      double time = e.get("duration").asDouble() / 1000;
      result.add(
          new InitialStop(
              stop, e.get("cost").asDouble(), time, e.get("distance").asDouble(), "walk"));
    }
    return result;
  }

  public static void main(String[] args) throws Exception {
    if (args.length != 7 && args.length != 8)
      throw new IllegalArgumentException(
          "SCHEDULE NETWORK REQUESTS OUTPUT THREADS WARMUP TRANSFER_SECONDS");
    var json = new ObjectMapper();
    var profile = args.length == 8 ? json.readTree(Path.of(args[7]).toFile()) : null;
    weightedProfile = profile != null;
    var out = Path.of(args[3]);
    Files.createDirectories(out);
    int threads = Integer.parseInt(args[4]), warmup = Integer.parseInt(args[5]);
    double buffer = Double.parseDouble(args[6]);
    long started = System.nanoTime();
    var config = ConfigUtils.createConfig(new SwissRailRaptorConfigGroup());
    config.transit().setUseTransit(true);
    var scenario = ScenarioUtils.createScenario(config);
    new MatsimNetworkReader(scenario.getNetwork()).readFile(args[1]);
    new TransitScheduleReader(scenario).readFile(args[0]);
    var schedule = scenario.getTransitSchedule();
    var originalTransfers =
        new ArrayList<Map.Entry<Id<TransitStopFacility>, Id<TransitStopFacility>>>();
    var iterator = schedule.getMinimalTransferTimes().iterator();
    while (iterator.hasNext()) {
      iterator.next();
      originalTransfers.add(Map.entry(iterator.getFromStopId(), iterator.getToStopId()));
    }
    if (profile == null)
      for (var edge : originalTransfers)
        schedule.getMinimalTransferTimes().remove(edge.getKey(), edge.getValue());
    var served = new HashSet<TransitStopFacility>();
    for (var line : schedule.getTransitLines().values())
      for (var service : line.getRoutes().values())
        for (var call : service.getStops()) served.add(call.getStopFacility());
    if (profile == null)
      for (var stop : served)
        schedule.getMinimalTransferTimes().set(stop.getId(), stop.getId(), buffer);
    else
      for (var edge : originalTransfers)
        if (!served.contains(schedule.getFacilities().get(edge.getKey()))
            || !served.contains(schedule.getFacilities().get(edge.getValue())))
          schedule.getMinimalTransferTimes().remove(edge.getKey(), edge.getValue());
    var staticConfig = new RaptorStaticConfig();
    staticConfig.setBeelineWalkConnectionDistance(0);
    staticConfig.setBeelineWalkSpeed(1.0);
    staticConfig.setBeelineWalkDistanceFactor(1.0);
    staticConfig.setMinimalTransferTime(buffer);
    staticConfig.setTransferWalkMargin(0);
    staticConfig.setUseCapacityConstraints(false);
    if (profile != null) {
      staticConfig.setBeelineWalkConnectionDistance(profile.get("transfer_radius_m").asDouble());
      staticConfig.setBeelineWalkSpeed(profile.get("effective_walk_speed_m_s").asDouble());
      staticConfig.setBeelineWalkDistanceFactor(profile.get("walk_distance_factor").asDouble());
      staticConfig.setTransferWalkMargin(profile.get("transfer_walk_margin_s").asDouble());
      for (var p : profile.get("mode_transfers"))
        staticConfig.addModeToModeTransferPenalty(
            p.get("from").asText(), p.get("to").asText(), p.get("cost").asDouble());
    }
    var data =
        SwissRailRaptorData.create(schedule, null, staticConfig, scenario.getNetwork(), null);
    // Compact the transfer ranges in place before any router exists. This is benchmark input
    // preparation,
    // not a change to the algorithm: only identical stop IDs can exchange passengers in this
    // protocol.
    int written = 0;
    if (profile == null)
      for (var stop : data.routeStops) {
        int first = stop.indexFirstTransfer, count = stop.countTransfers, start = written;
        for (int i = 0; i < count; i++) {
          var transfer = data.transfers[first + i];
          if (data.routeStops[transfer.fromRouteStop]
              .routeStop
              .getStopFacility()
              .getId()
              .equals(data.routeStops[transfer.toRouteStop].routeStop.getStopFacility().getId()))
            data.transfers[written++] = transfer;
        }
        stop.indexFirstTransfer = start;
        stop.countTransfers = written - start;
      }
    if (profile == null) Arrays.fill(data.transfers, written, data.transfers.length, null);
    else {
      var graph = new TreeMap<String, Map<String, Object>>();
      for (var e : data.transfers) {
        String from =
            data.routeStops[e.fromRouteStop].routeStop.getStopFacility().getId().toString();
        String to = data.routeStops[e.toRouteStop].routeStop.getStopFacility().getId().toString();
        graph.put(
            from.length() + ":" + from + to,
            Map.of("from", from, "to", to, "duration", e.transferTime * 1000));
      }
      // A physical same-stop buffer is needed even when Raptor prunes every associated route pair.
      for (var stop : served) {
        String id = stop.getId().toString();
        double time = schedule.getMinimalTransferTimes().get(stop.getId(), stop.getId(), buffer);
        graph.put(
            id.length() + ":" + id + id,
            Map.of("from", id, "to", id, "duration", Math.round(Math.ceil(time) * 1000)));
      }
      json.writeValue(out.resolve("transfers.json").toFile(), graph.values());
    }
    var params =
        new RaptorParameters(ConfigUtils.addOrGetModule(config, SwissRailRaptorConfigGroup.class));
    params.setBeelineWalkSpeed(1.0);
    params.setMarginalUtilityOfTravelTime_utl_s("walk", -1);
    params.setMarginalUtilityOfTravelTime_utl_s("pt", -1);
    params.setMarginalUtilityOfWaitingPt_utl_s(-1);
    params.setTransferPenaltyFixCostPerTransfer(0);
    params.setTransferPenaltyPerTravelTimeHour(0);
    params.setTransferPenaltyMinimum(0);
    params.setTransferPenaltyMaximum(0);
    params.setUseTransportModeUtilities(false);
    if (profile != null) {
      params.setMarginalUtilityOfTravelTime_utl_s(
          "walk", -profile.get("walk_per_hour").asDouble() / 3600);
      params.setMarginalUtilityOfWaitingPt_utl_s(-profile.get("wait_per_hour").asDouble() / 3600);
      for (var line : schedule.getTransitLines().values())
        for (var service : line.getRoutes().values()) {
          String mode = service.getTransportMode();
          var rate = profile.get("modes_per_hour").get(mode);
          params.setMarginalUtilityOfTravelTime_utl_s(
              mode,
              -(rate == null ? profile.get("other_per_hour").asDouble() : rate.asDouble()) / 3600);
        }
      params.setTransferPenaltyFixCostPerTransfer(profile.get("transfer_base").asDouble());
      params.setTransferPenaltyPerTravelTimeHour(
          profile.get("transfer_per_travel_hour").asDouble());
      params.setTransferPenaltyMinimum(Double.parseDouble(profile.get("transfer_min").asText()));
      params.setTransferPenaltyMaximum(Double.parseDouble(profile.get("transfer_max").asText()));
      params.setUseTransportModeUtilities(true);
    }
    double load = (System.nanoTime() - started) / 1e9;
    var requests = new ArrayList<Request>();
    for (var text : Files.readAllLines(Path.of(args[2]))) {
      var row = json.readTree(text);
      var from =
          schedule
              .getFacilities()
              .get(Id.create(row.get("from").asText(), TransitStopFacility.class));
      var to =
          schedule
              .getFacilities()
              .get(Id.create(row.get("to").asText(), TransitStopFacility.class));
      requests.add(
          new Request(
              row.get("id").asText(),
              from,
              to,
              seconds(row.get("departure").asText()),
              endpoints(row.get("access"), from, schedule),
              endpoints(row.get("egress"), to, schedule)));
    }
    if (requests.isEmpty()) throw new IllegalArgumentException("No requests");
    var router = core(data);
    for (int i = 0; i < warmup; i++) route(router, requests.get(i % requests.size()), params);
    var results = new ArrayList<RaptorRoute>();
    var nanos = new ArrayList<Long>();
    long serialStart = System.nanoTime();
    for (var request : requests) {
      long now = System.nanoTime();
      var result = route(router, request, params);
      nanos.add(System.nanoTime() - now);
      results.add(result);
    }
    double serial = (System.nanoTime() - serialStart) / 1e9;
    var pool = Executors.newFixedThreadPool(threads);
    var local = ThreadLocal.withInitial(() -> core(data));
    double parallel;
    try {
      var warm = new ArrayList<Callable<RaptorRoute>>();
      for (int i = 0; i < warmup; i++) {
        var request = requests.get(i % requests.size());
        warm.add(() -> route(local.get(), request, params));
      }
      for (var result : pool.invokeAll(warm)) result.get();
      var tasks = new ArrayList<Callable<RaptorRoute>>();
      for (var request : requests) tasks.add(() -> route(local.get(), request, params));
      long now = System.nanoTime();
      var batch = pool.invokeAll(tasks);
      var batchResults = new ArrayList<RaptorRoute>();
      for (var result : batch) batchResults.add(result.get());
      parallel = (System.nanoTime() - now) / 1e9;
      for (int i = 0; i < batch.size(); i++)
        if (!Objects.equals(describe(results.get(i)), describe(batchResults.get(i))))
          throw new IllegalStateException("parallel result differs at " + i);
    } finally {
      pool.shutdown();
    }
    try (var writer = Files.newBufferedWriter(out.resolve("swiss_results.jsonl"))) {
      for (int i = 0; i < requests.size(); i++) {
        var row = new LinkedHashMap<String, Object>();
        row.put("id", requests.get(i).id);
        row.put("runtime_ns", nanos.get(i));
        row.put("journey", describe(results.get(i)));
        writer.write(json.writeValueAsString(row));
        writer.newLine();
      }
    }
    json.writeValue(
        out.resolve("swiss_metadata.json").toFile(),
        Map.of(
            "engine",
            "SwissRailRaptorCore MATSim 2026.0",
            "load_seconds",
            load,
            "serial_seconds",
            serial,
            "parallel_seconds",
            parallel,
            "threads",
            threads,
            "queries",
            requests.size(),
            "warmup_per_mode",
            warmup,
            "parallel_matches_serial",
            true,
            "java",
            System.getProperty("java.version")));
    System.out.println("Benchmark complete: " + serial + " s serial; " + parallel + " s parallel");
  }
}
