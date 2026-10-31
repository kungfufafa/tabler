import { createContext, useContext } from "react";

/**
 * Whether the rows shown can lack a field (`rowsCanLackFields` of their session): the grid, Quick
 * Look, the editors and the inspector then read `Other{missing}` as a missing field
 * (`isMissingValue`), not as a value of a type named "missing". Set per tab (`TabBody`) and for the
 * inspector; off by default, as on a core session.
 */
export const LacksFieldsContext = createContext(false);

export const useLacksFields = (): boolean => useContext(LacksFieldsContext);
