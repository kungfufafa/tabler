import type { LaneModule } from "../types";
import { mongoConsole } from "./console";
import { mongoUi } from "./form";
import { MongoInspector } from "./MongoInspector";
import { mongoObjectMenu } from "./menu";

/**
 * The renderer of the mongodb lane (MongoDB; docs/drivers/mongodb.md): the connection form with
 * its `mongodb://` and `mongodb+srv://` URLs, the mongosh-subset console (split and classified as
 * the backend does), the document inspector, and a collection's menu. The grid and the Structure
 * tab are the app's own: the session's sampled columns, `Other{objectId}` and missing cells fit
 * them.
 */
const lane: LaneModule = {
  kinds: { mongodb: mongoUi },
  drivers: {
    mongodb: { console: mongoConsole, Inspector: MongoInspector, objectMenu: mongoObjectMenu },
  },
};

export default lane;
