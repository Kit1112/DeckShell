// QAM Friends bridge (one Runtime.evaluate expression, returns JSON).
// Sources (snapshot sp.js, changelist 11146165):
//   friends:    friendStore.allFriends (FriendStore.all_friends)                     -> FriendsListTabPanel "Friends" groups
//   favorites:  friendStore.m_FriendsUIFriendStore.FavoritesStore.favorites        -> FavoritesStore.m_rgFavorites ({friend}|{clan}|{chat})
//   groups:     g_FriendsUIApp.ChatStore.currentChatRoomGroups                    -> ChatStore.currentChatRoomGroups (current user is a member)
//   recent:     g_FriendsUIApp.ChatStore.GetRecentChats()                         -> ChatStore.GetRecentChats (friend chats + group chats with a last message)
//   invites:    g_FriendsUIApp.FriendStore.GetPendingInviteCount()                -> the Friends header invites button (shown when > 0)
// Every field is read defensively: a missing store yields an empty list, never a throw (the Rust side keeps the previous list).
(()=>{
  const U=window.g_FriendsUIApp||{};
  const FS=window.friendStore||U.FriendStore||{};
  const CS=U.ChatStore||U.m_ChatStore||null;
  const persona=f=>f.m_persona||f.persona||{};
  const fr=f=>{
    const p=persona(f);
    return {
      n:f.m_strNickname||p.m_strPlayerName||f.display_name||'',
      s:p.m_ePersonaState||0,
      g:String(p.m_gameid||'0')==='0'?'':(p.m_strGameExtraInfo||((window.appStore&&appStore.GetAppOverviewByAppID(Number(p.m_gameid)))||{}).display_name||'?'),
      a:f.accountid||0,
      av:!!p.m_strAvatarHash,
      u:p.avatar_url_medium||''
    };
  };
  const grp=g=>({k:'g',n:g.name||'',id:String(g.GetGroupID?g.GetGroupID():(g.group_id||'')),c:g.has_unread_messages?1:0,m:g.has_unread_mention?1:0,p:g.has_unread_priority_messages?1:0});
  const list=(fn,dflt)=>{try{const v=fn();return Array.isArray(v)?v:(v&&typeof v.slice==='function'?v.slice():dflt)}catch(e){return dflt}};
  const friends=list(()=>FS.allFriends,[]).map(fr);
  const favRaw=list(()=>(FS.m_FriendsUIFriendStore&&FS.m_FriendsUIFriendStore.FavoritesStore||(U.FriendStore&&U.FriendStore.FavoritesStore)).favorites,[]);
  const favorites=favRaw.map(e=>{
    if(e.friend) return Object.assign({k:'f'},fr(e.friend));
    if(e.chat) return grp(e.chat);
    if(e.clan) return {k:'c',n:e.clan.name||e.clan.m_strName||'',id:String(e.clan.clanid||e.clan.m_steamid||''),c:0,m:0,p:0};
    return null;
  }).filter(Boolean);
  const groups=list(()=>CS&&CS.currentChatRoomGroups,[]).map(grp);
  const recent=list(()=>CS&&CS.GetRecentChats&&CS.GetRecentChats(),[]).map(x=>{
    if(x.chat_partner) return Object.assign({k:'f',c:x.unread_message_count||0,p:x.has_unread_priority_messages?1:0,m:x.has_unread_mention?1:0},fr(x.chat_partner));
    return grp(x);
  });
  let invites=0;
  try{invites=(FS.GetPendingInviteCount?FS.GetPendingInviteCount():((U.FriendStore||{}).GetPendingInviteCount?U.FriendStore.GetPendingInviteCount():0))||0}catch(e){invites=0}
  return {
    ready:FS.friends_list_ready!==false,
    friends:friends,
    favorites:favorites,
    groups:groups,
    recent:recent,
    invites:invites
  };
})()
