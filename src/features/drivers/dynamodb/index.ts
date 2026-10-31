import type { LaneModule } from "../types";
import { dynamoConsole } from "./console";
import { DynamoInspector } from "./DynamoInspector";
import { dynamoUi } from "./form";
import { dynamoObjectMenu } from "./menu";

/**
 * The renderer of the dynamodb lane (DynamoDB; docs/drivers/dynamodb.md): the connection form
 * with its credential sources and `dynamodb://` URLs, the PartiQL and API console (split and
 * classified as the backend does), the item inspector with its DynamoDB JSON ↔ plain JSON
 * toggle, and a table's menu (Table Info…, Browse Index…). The grid and the Structure tab are
 * the app's own: the session's sampled attributes, cursor pages and missing cells fit them. No
 * SQL dialect: the console is native.
 */
const lane: LaneModule = {
  kinds: { dynamodb: dynamoUi },
  drivers: {
    dynamodb: {
      console: dynamoConsole,
      Inspector: DynamoInspector,
      objectMenu: dynamoObjectMenu,
    },
  },
};

export default lane;
