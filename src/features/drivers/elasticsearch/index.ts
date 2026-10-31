import type { LaneModule } from "../types";
import { esConsole } from "./console";
import { ElasticInspector } from "./ElasticInspector";
import { elasticUi } from "./form";
import { esObjectMenu } from "./menu";

/**
 * The renderer of the elasticsearch lane (Elasticsearch and OpenSearch;
 * docs/drivers/elasticsearch.md): the connection form with its `elasticsearch://` and
 * `opensearch://` URLs, the Kibana-style console with SQL (split and classified as the backend
 * does), the hit inspector, and an index's menu. The grid and the Structure tab are the app's
 * own: the session's flattened mapping, cursor pages and missing cells fit them.
 */
const lane: LaneModule = {
  kinds: { elasticsearch: elasticUi },
  drivers: {
    elasticsearch: { console: esConsole, Inspector: ElasticInspector, objectMenu: esObjectMenu },
  },
};

export default lane;
