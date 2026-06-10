export { renderL1, type L1Options, type L1Result } from "./l1/index.js";
export { wordToHTML } from "./l1/word.js";
export { slideToSVG, type SlideSvgOptions } from "./l1/ppt.js";
export {
  L2PreviewEngine,
  createDefaultRasterRenderer,
  rasterKey,
  FallbackRasterRenderer,
  SofficeRasterRenderer,
  findSoffice,
  type RasterRenderer,
  type RasterRequest,
  type RasterResult,
} from "./l2/index.js";
