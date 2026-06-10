/** id 生成。可注入工厂以便测试确定性。 */

export type IdFactory = (prefix: string) => string;

let counter = 0;

/** 默认 id:前缀 + 时间基 + 自增 + 随机,跨会话足够唯一。 */
export const defaultIdFactory: IdFactory = (prefix: string) => {
  counter = (counter + 1) % 0xffffff;
  const rand = Math.random().toString(36).slice(2, 8);
  const seq = counter.toString(36);
  return `${prefix}_${seq}${rand}`;
};

/** 确定性工厂,供单测使用。 */
export function createSeqIdFactory(): IdFactory {
  let n = 0;
  return (prefix: string) => `${prefix}_${++n}`;
}
